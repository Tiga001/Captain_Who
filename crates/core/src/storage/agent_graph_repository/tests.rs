use super::*;
use crate::storage::{
    agent_collaboration_event_repository, chat_repository, migrations,
    models::{ChatConversationMetaRecord, ChatMessageRecord},
};
use crate::{
    AcknowledgeAgentTaskAndWakeInput, AgentBuiltinExecutionPermission, AgentCommandPermission,
    AgentCommandSafetyPolicy, AgentDisplayStatus, AgentEffectivePermissionSnapshot,
    AgentGraphError, AgentLifecycle, AgentMailboxDeliveryStatus, AgentMailboxKind,
    AgentModelSelectionSnapshot, AgentNodeRecord, AgentPatchPermission, AgentPermissions,
    AgentReadPermission, AgentTreeStoppedWakeSettlementOutcome, AgentWakeRecoveryAction,
    AgentWakeStatus, AgentWritePermission, ConversationMessageOrigin, CreateAgentNodeInput,
    EnqueueAgentMessageInput, EnqueueAgentWakeInput, EnsureRootAgentInput,
    FinishAgentTurnResultInput, FinishAgentWakeWithResultInput, IdempotentCreate,
    InterruptAgentExecutionOutcome, SendAgentMessageRequest, UndispatchedAgentInterrupt,
    AGENT_EFFECTIVE_PERMISSION_SNAPSHOT_SCHEMA_VERSION,
};
use rusqlite::{params, Connection, TransactionBehavior};

fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    connection
}

fn insert_project(connection: &Connection, project_id: &str) {
    connection
        .execute(
            "INSERT INTO projects (id, name, created_at, pinned_at, updated_at)
                 VALUES (?1, ?1, 1, NULL, 1)",
            [project_id],
        )
        .unwrap();
}

fn insert_conversation(connection: &Connection, id: &str, project_id: Option<&str>) {
    connection
        .execute(
            "INSERT INTO conversations (
                     id, project_id, model_id, title, created_at, updated_at,
                     pinned_at, archived_at, unread_at
                 ) VALUES (?1, ?2, 'model-a', ?1, 1, 1, NULL, NULL, NULL)",
            params![id, project_id],
        )
        .unwrap();
}

fn model_snapshot(model_id: &str) -> AgentModelSelectionSnapshot {
    AgentModelSelectionSnapshot {
        model_config_id: model_id.to_string(),
        display_name: format!("Model {model_id}"),
        supports_image: false,
        effective_context_window_tokens: 64_000,
        model_settings_configuration_revision: "model-settings-v1:test".to_string(),
        provider_connection_revision: "provider-connection-v1:test".to_string(),
        provider_protocol_revision: "provider-protocol-v1:test".to_string(),
    }
}

fn ensure_root(
    connection: &mut Connection,
    agent_id: &str,
    conversation_id: &str,
) -> AgentNodeRecord {
    ensure_root_agent(
        connection,
        &EnsureRootAgentInput {
            agent_id: agent_id.to_string(),
            conversation_id: conversation_id.to_string(),
            creation_request_id: format!("ensure-{agent_id}"),
            task_name: "Root".to_string(),
        },
        10,
    )
    .unwrap()
    .record()
    .clone()
}

fn child_input(
    agent_id: &str,
    root_agent_id: &str,
    parent_agent_id: &str,
    conversation_id: &str,
    task_name: &str,
    task_path: &str,
) -> CreateAgentNodeInput {
    CreateAgentNodeInput {
        agent_id: agent_id.to_string(),
        root_agent_id: root_agent_id.to_string(),
        parent_agent_id: parent_agent_id.to_string(),
        conversation_id: conversation_id.to_string(),
        creation_request_id: format!("spawn-{agent_id}"),
        task_name: task_name.to_string(),
        task_path: task_path.to_string(),
        template_snapshot: None,
        model_snapshot: model_snapshot("model-a"),
    }
}

fn setup_tree() -> Connection {
    let mut connection = connection();
    insert_project(&connection, "project-a");
    for conversation in [
        "conversation-root",
        "conversation-child",
        "conversation-grand",
    ] {
        insert_conversation(&connection, conversation, Some("project-a"));
    }
    ensure_root(&mut connection, "agent-root", "conversation-root");
    create_agent_node(
        &mut connection,
        &child_input(
            "agent-child",
            "agent-root",
            "agent-root",
            "conversation-child",
            "review",
            "/root/review",
        ),
        11,
    )
    .unwrap();
    connection
}

fn full_permissions() -> AgentPermissions {
    AgentPermissions {
        read: AgentReadPermission::All,
        write: AgentWritePermission::All,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::FullAccess,
        patch: AgentPatchPermission::AutoApprove,
        builtin_execution: AgentBuiltinExecutionPermission::AutoApprove,
    }
}

fn record_permissions_for_test_turn(
    connection: &mut Connection,
    agent_id: &str,
    conversation_id: &str,
    suffix: &str,
    permissions: AgentPermissions,
    timestamp: i64,
) -> AgentEffectivePermissionSnapshot {
    let assistant_message_id = format!("assistant-permissions-{suffix}");
    let run_id = format!("run-permissions-{suffix}");
    connection
        .execute(
            "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     ?1, ?2, 'assistant', 'permission fixture', 'pending', ?3,
                     (SELECT COALESCE(MAX(position), -1) + 1 FROM messages
                      WHERE conversation_id = ?2)
                 )",
            params![&assistant_message_id, conversation_id, timestamp],
        )
        .unwrap();
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        &run_id,
        conversation_id,
        &assistant_message_id,
    );
    crate::storage::conversation_trace_repository::append_in_progress_trace(
        connection, &trace, timestamp, timestamp,
    )
    .unwrap();
    let snapshot = record_agent_effective_permissions_for_active_turn(
        connection,
        agent_id,
        conversation_id,
        &run_id,
        &assistant_message_id,
        permissions,
        timestamp,
    )
    .unwrap();
    connection
        .execute(
            "UPDATE conversation_turn_traces
                 SET terminal_status = 'completed', completed_at = ?1, updated_at = ?1
                 WHERE run_id = ?2 AND terminal_status = 'in_progress'",
            params![timestamp.saturating_add(1), &run_id],
        )
        .unwrap();
    snapshot
}

fn message_input(
    suffix: &str,
    sender: &str,
    recipient: &str,
    kind: AgentMailboxKind,
) -> EnqueueAgentMessageInput {
    EnqueueAgentMessageInput {
        message_id: format!("message-{suffix}"),
        root_agent_id: "agent-root".to_string(),
        sender_agent_id: sender.to_string(),
        recipient_agent_id: recipient.to_string(),
        request_id: format!("request-message-{suffix}"),
        kind,
        content: format!("payload {suffix}"),
        projection_message_id: format!("projection-{suffix}"),
    }
}

fn chat_message(id: &str, role: &str, content: &str, created_at: i64) -> ChatMessageRecord {
    ChatMessageRecord {
        human_interaction_response: None,
        id: id.to_string(),
        role: role.to_string(),
        content: content.to_string(),
        created_at,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        folder_references_json: None,
        agent_run_json: None,
        ui_state_json: None,
    }
}

fn wake_input(suffix: &str) -> EnqueueAgentWakeInput {
    EnqueueAgentWakeInput {
        wake_id: format!("wake-{suffix}"),
        root_agent_id: "agent-root".to_string(),
        agent_id: "agent-child".to_string(),
        requester_agent_id: "agent-root".to_string(),
        request_id: format!("request-wake-{suffix}"),
        source_agent_message_id: None,
    }
}

fn add_grandchild(connection: &mut Connection) -> AgentNodeRecord {
    create_agent_node(
        connection,
        &child_input(
            "agent-grand",
            "agent-root",
            "agent-child",
            "conversation-grand",
            "details",
            "/root/review/details",
        ),
        12,
    )
    .unwrap()
    .record()
    .clone()
}

mod delivery_lifecycle;
mod mailbox_projection;
mod nodes_permissions;
mod result_recovery;
mod tree_cancellation;
