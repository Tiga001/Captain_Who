use crate::storage::models::ChatConversationRecord;
use crate::AgentFileChangeSnapshot;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Reads only the public FileChange snapshot columns. Draft/base bodies, private observations,
/// arguments digests and permission bindings stay in their existing execution repositories.
/// The caller owns the same read transaction as messages and Trace; no writer is acquired here.
pub(super) fn attach_file_change_history(
    connection: &Connection,
    conversation: &mut ChatConversationRecord,
) -> Result<(), String> {
    let messages = conversation
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == "assistant")
        .map(|(index, message)| (message.id.clone(), index))
        .collect::<HashMap<_, _>>();
    if messages.is_empty() {
        return Ok(());
    }
    let ids = serde_json::to_string(&messages.keys().collect::<Vec<_>>())
        .map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT trace.assistant_message_id, change.run_id,
                change.schema_version, change.id, change.conversation_id, change.project_id,
                change.file_path, change.operation, change.strategy, change.status,
                change.base_revision, change.additions, change.deletions, change.line_count,
                change.byte_count, change.mutation_count, change.next_mutation_index,
                change.stats_final, change.summary, change.created_at, change.updated_at
         FROM agent_file_changes AS change
         JOIN conversation_turn_traces AS trace
           ON trace.conversation_id = change.conversation_id AND trace.run_id = change.run_id
         WHERE change.conversation_id = ?1
           AND trace.assistant_message_id IN (SELECT value FROM json_each(?2))
           AND change.source_tool_name = 'apply_patch'
           AND EXISTS (
               SELECT 1 FROM conversation_turn_trace_items AS item
               WHERE item.assistant_message_id = trace.assistant_message_id
                 AND item.item_kind = 'tool_call'
                 AND json_extract(item.item_json, '$.callId') = change.source_tool_call_id
                 AND json_extract(item.item_json, '$.tool') = change.source_tool_name
                 AND json_extract(item.item_json, '$.provenance.type') = 'builtin'
                 AND json_extract(item.item_json, '$.provenance.toolName') = change.source_tool_name
           )
         ORDER BY change.created_at, change.id",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement.query_map(rusqlite::params![conversation.id, ids], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, json!({
            "schemaVersion":row.get::<_, u32>(2)?, "transactionId":row.get::<_, String>(3)?,
            "conversationId":row.get::<_, String>(4)?, "projectId":row.get::<_, Option<String>>(5)?,
            "filePath":row.get::<_, String>(6)?, "operation":row.get::<_, String>(7)?,
            "updateStrategy":row.get::<_, Option<String>>(8)?, "status":row.get::<_, String>(9)?,
            "baseRevision":row.get::<_, Option<String>>(10)?, "additions":row.get::<_, u64>(11)?,
            "deletions":row.get::<_, u64>(12)?, "lineCount":row.get::<_, u64>(13)?,
            "byteCount":row.get::<_, u64>(14)?, "mutationCount":row.get::<_, u64>(15)?,
            "nextMutationIndex":row.get::<_, u64>(16)?, "statsFinal":row.get::<_, bool>(17)?,
            "summary":row.get::<_, Option<String>>(18)?, "createdAt":row.get::<_, i64>(19)?,
            "updatedAt":row.get::<_, i64>(20)?,
        })))
    }).map_err(|error| error.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|error| error.to_string())?;
    let mut grouped: HashMap<(String, String), Vec<Value>> = HashMap::new();
    for (message_id, run_id, snapshot) in rows {
        // Same validator used by file_change_support::file_change_snapshot, without loading
        // execution-only AgentFileChangeRecord bodies merely to discard them again.
        let snapshot: AgentFileChangeSnapshot = serde_json::from_value(snapshot)
            .map_err(|error| format!("invalid durable FileChange history snapshot: {error}"))?;
        grouped
            .entry((message_id, run_id))
            .or_default()
            .push(serde_json::to_value(snapshot).map_err(|error| error.to_string())?);
    }
    for ((message_id, run_id), snapshots) in grouped {
        let message =
            &mut conversation.messages[*messages.get(&message_id).expect("selected message")];
        let Some(raw) = message.agent_run_json.as_deref() else {
            continue;
        };
        let mut projection: Value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
        if projection["runId"].as_str() != Some(run_id.as_str()) {
            continue;
        }
        let Some(run) = projection.as_object_mut() else {
            continue;
        };
        let changes = run.entry("fileChanges").or_insert_with(|| json!([]));
        let Some(changes) = changes.as_array_mut() else {
            continue;
        };
        let mut positions = changes
            .iter()
            .enumerate()
            .filter_map(|(index, change)| {
                change["transactionId"]
                    .as_str()
                    .map(|id| (id.to_owned(), index))
            })
            .collect::<HashMap<_, _>>();
        for snapshot in snapshots {
            let id = snapshot["transactionId"]
                .as_str()
                .expect("validated transaction ID")
                .to_owned();
            if let Some(&index) = positions.get(&id) {
                changes[index] = snapshot;
            } else {
                positions.insert(id, changes.len());
                changes.push(snapshot);
            }
        }
        message.agent_run_json =
            Some(serde_json::to_string(&projection).map_err(|error| error.to_string())?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{
        chat_repository, conversation_trace_repository, file_change_repository,
        models::{AgentFileChangeRecord, ChatMessageRecord},
        service::StorageService,
    };
    use crate::{
        AgentApprovalStatus, AgentToolIdentity, ConversationTurnTrace, ConversationTurnTraceItem,
        ConversationTurnTraceTerminalStatus,
    };
    use std::sync::Arc;

    fn fixture() -> (tempfile::TempDir, StorageService) {
        let directory = tempfile::tempdir().unwrap();
        let service = StorageService::open_with_model_credentials(
            &directory.path().join("file-change.sqlite"),
            Arc::new(crate::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap();
        service.save_conversation(ChatConversationRecord {
            id:"conversation".into(),project_id:None,model_id:None,title:"synthetic".into(),created_at:1,updated_at:1,pinned_at:None,archived_at:None,unread_at:None,
            messages:vec![ChatMessageRecord { id:"assistant".into(),role:"assistant".into(),content:String::new(),created_at:1,status:Some("sent".into()),attachments:vec![],folder_references_json:None,human_interaction_response:None,ui_state_json:None,agent_run_json:Some(json!({"runId":"run","fileChanges":[{"transactionId":"change","status":"waiting_approval","additions":0}]}).to_string()) }],
        }).unwrap();
        let trace = ConversationTurnTrace {
            schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run".into(),
            conversation_id: "conversation".into(),
            assistant_message_id: "assistant".into(),
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            terminal_error: None,
            truncated: false,
            items: vec![ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: "begin".into(),
                tool: "apply_patch".into(),
                provenance: AgentToolIdentity::Builtin {
                    tool_name: "apply_patch".into(),
                },
                operation: json!({"request":{"action":"begin","operation":"create","filePath":"result.txt"}}),
                approval_status: AgentApprovalStatus::NotRequired,
                truncated: false,
            }],
        };
        let mut connection = service.state.connection().unwrap();
        conversation_trace_repository::replace_trace(&mut connection, &trace, 1, 2).unwrap();
        let change = AgentFileChangeRecord {
            schema_version: 1,
            id: "change".into(),
            conversation_id: "conversation".into(),
            project_id: None,
            run_id: "run".into(),
            source_tool_name: "apply_patch".into(),
            source_tool_call_id: "begin".into(),
            source_tool_arguments_digest: "digest".into(),
            permission_revision: "permission".into(),
            tool_set_revision: "tools".into(),
            provider_wire_revision: "wire".into(),
            observation_id: "observation".into(),
            observation_json: "{}".into(),
            file_path: "result.txt".into(),
            operation: "create".into(),
            strategy: None,
            status: "waiting_approval".into(),
            base_revision: None,
            base_content: "private base".repeat(1000),
            content: "private draft".repeat(1000),
            draft_revision: 1,
            next_mutation_index: 0,
            additions: 1,
            deletions: 0,
            line_count: 1,
            byte_count: 12,
            mutation_count: 0,
            stats_final: true,
            summary: None,
            final_action_id: Some("action".into()),
            final_action_arguments_digest: Some("action-digest".into()),
            final_permission_revision: Some("permission".into()),
            final_tool_set_revision: Some("tools".into()),
            final_provider_wire_revision: Some("wire".into()),
            created_at: 1,
            updated_at: 2,
            expires_at: 100,
        };
        file_change_repository::insert_file_change(&connection, &change).unwrap();
        drop(connection);
        (directory, service)
    }

    fn projected(connection: &Connection) -> Value {
        let mut conversation = chat_repository::get_active_conversation(connection, "conversation")
            .unwrap()
            .unwrap();
        attach_file_change_history(connection, &mut conversation).unwrap();
        serde_json::from_str(conversation.messages[0].agent_run_json.as_deref().unwrap()).unwrap()
    }

    #[test]
    fn external_approval_settlement_refreshes_stats_from_one_read_cut_without_draft_bodies() {
        let (_directory, service) = fixture();
        let mut reader = service.state.read_connection().unwrap();
        let snapshot = reader.transaction().unwrap();
        assert_eq!(
            projected(&snapshot)["fileChanges"][0]["status"],
            "waiting_approval"
        );
        // A separate request owns the writer; nested acquisition on this reader's thread is
        // deliberately forbidden by the storage execution contract.
        std::thread::scope(|scope| {
            scope.spawn(|| {
            service.state.connection().unwrap().execute("UPDATE agent_file_changes SET status='applied', additions=7, updated_at=3 WHERE id='change'", []).unwrap();
        }).join().unwrap()
        });
        assert_eq!(
            projected(&snapshot)["fileChanges"][0]["status"],
            "waiting_approval"
        );
        snapshot.commit().unwrap();
        let current = projected(&reader);
        assert_eq!(current["fileChanges"][0]["status"], "applied");
        assert_eq!(current["fileChanges"][0]["additions"], 7);
        assert_eq!(current["fileChanges"].as_array().unwrap().len(), 1);
        assert!(!current.to_string().contains("private"));
    }

    #[test]
    fn source_call_and_turn_identity_are_required_before_overlaying_history() {
        let (_directory, service) = fixture();
        let connection = service.state.connection().unwrap();
        connection.execute("UPDATE agent_file_changes SET source_tool_call_id='other', status='applied' WHERE id='change'", []).unwrap();
        assert_eq!(
            projected(&connection)["fileChanges"][0]["status"],
            "waiting_approval"
        );
        connection.execute("UPDATE agent_file_changes SET source_tool_call_id='begin', run_id='other' WHERE id='change'", []).unwrap();
        assert_eq!(
            projected(&connection)["fileChanges"][0]["status"],
            "waiting_approval"
        );
    }
}
