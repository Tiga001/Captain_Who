use super::*;
use mycopilot_core::command::CommandAuthorizationSource;
use mycopilot_core::storage::agent_command_session_repository::{
    AgentCommandSessionCreate, AGENT_COMMAND_SESSION_SCHEMA_VERSION,
};
use mycopilot_core::storage::models::{
    ChatConversationRecord, ChatMessageRecord, ConversationForkPoint, ForkConversationRequest,
};
use mycopilot_core::{AgentCommandSessionSnapshot, AgentCommandSessionStatus};

#[test]
fn latest_fork_request_is_host_resolved_and_rejects_client_cursor() {
    let request = serde_json::from_value::<ForkConversationRequest>(serde_json::json!({
        "requestId":"latest", "sourceConversationId":"source", "forkPoint":{"kind":"latest"}
    }))
    .unwrap();
    assert_eq!(request.fork_point, ConversationForkPoint::Latest {});
    assert!(serde_json::from_value::<ForkConversationRequest>(serde_json::json!({
        "requestId":"latest", "sourceConversationId":"source", "forkPoint":{"kind":"latest", "assistantMessageId":"untrusted"}
    })).is_err());
}

#[test]
fn manual_compaction_boundary_request_accepts_only_the_durable_operation_identity() {
    let request = serde_json::from_value::<ForkConversationRequest>(serde_json::json!({
        "requestId":"manual-boundary", "sourceConversationId":"source",
        "forkPoint":{"kind":"manual_compaction_boundary", "operationId":"manual-compaction-operation"}
    })).unwrap();
    assert_eq!(
        request.fork_point,
        ConversationForkPoint::ManualCompactionBoundary {
            operation_id: "manual-compaction-operation".into(),
        }
    );
    for field in [
        "assistantMessageId",
        "summaryId",
        "modelId",
        "coveredThroughMessageId",
    ] {
        let mut params = serde_json::json!({
            "requestId":"manual-boundary", "sourceConversationId":"source",
            "forkPoint":{"kind":"manual_compaction_boundary", "operationId":"manual-compaction-operation"}
        });
        params["forkPoint"][field] = serde_json::json!("untrusted");
        assert!(
            serde_json::from_value::<ForkConversationRequest>(params).is_err(),
            "{field}"
        );
    }
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
    let agent_service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let response = handle_request(
        &storage,
        &agent_service,
        notifications,
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: JsonRpcId::Number(17),
            method: STORAGE_FORK_CONVERSATION_METHOD.into(),
            params: Some(serde_json::json!({
                "requestId":"manual-boundary", "sourceConversationId":"missing-conversation",
                "forkPoint":{"kind":"manual_compaction_boundary", "operationId":"manual-compaction-operation"}
            })),
        },
    );
    assert_eq!(response["error"]["code"], -32000, "{response}");
    assert_eq!(response["error"]["message"], "原任务不存在。");
}

#[test]
fn fork_request_accepts_camel_case_assistant_reply_point() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
    let agent_service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();

    let response = handle_request(
        storage.as_ref(),
        &agent_service,
        notifications,
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: JsonRpcId::Number(15),
            method: STORAGE_FORK_CONVERSATION_METHOD.to_string(),
            params: Some(serde_json::json!({
                "requestId": "fork-assistant-reply-request",
                "sourceConversationId": "missing-conversation",
                "forkPoint": {
                    "kind": "assistant_reply",
                    "assistantMessageId": "assistant-1"
                }
            })),
        },
    );

    assert_eq!(response["error"]["code"], -32000, "{response}");
    assert_eq!(response["error"]["message"], "原任务不存在。");
}

#[test]
fn fork_request_accepts_camel_case_provider_transition_boundary_point() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
    let agent_service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();

    let response = handle_request(
        storage.as_ref(),
        &agent_service,
        notifications,
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: JsonRpcId::Number(16),
            method: STORAGE_FORK_CONVERSATION_METHOD.to_string(),
            params: Some(serde_json::json!({
                "requestId": "fork-provider-transition-request",
                "sourceConversationId": "missing-conversation",
                "forkPoint": {
                    "kind": "provider_transition_boundary",
                    "operationId": "provider-transition-operation-1"
                }
            })),
        },
    );

    assert_eq!(response["error"]["code"], -32000, "{response}");
    assert_eq!(response["error"]["message"], "原任务不存在。");
}

#[test]
fn fork_request_reports_active_command_as_structured_domain_error() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
    let agent_service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-fork-rpc".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "fork rpc".to_string(),
            messages: vec![
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: "user-fork-rpc".to_string(),
                    role: "user".to_string(),
                    content: "run it".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: "assistant-fork-rpc".to_string(),
                    role: "assistant".to_string(),
                    content: "running".to_string(),
                    created_at: 2,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    folder_references_json: None,
                    agent_run_json: Some(
                        serde_json::json!({
                            "runId": "run-fork-rpc",
                            "status": "completed"
                        })
                        .to_string(),
                    ),
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .create_agent_command_session(&AgentCommandSessionCreate {
            snapshot: AgentCommandSessionSnapshot {
                schema_version: AGENT_COMMAND_SESSION_SCHEMA_VERSION,
                session_id: "cmd_000000000000000000000000000000d1".to_string(),
                conversation_id: "conversation-fork-rpc".to_string(),
                assistant_message_id: "assistant-fork-rpc".to_string(),
                origin_run_id: "run-fork-rpc".to_string(),
                call_id: "call-fork-rpc".to_string(),
                project_id: None,
                command: "python3 app.py".to_string(),
                cwd: "/tmp".to_string(),
                command_digest:
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                        .to_string(),
                status: AgentCommandSessionStatus::Running,
                started_at: 3,
                ended_at: None,
                exit_code: None,
                latest_sequence: 0,
                output_truncated: false,
                outputs: Vec::new(),
                artifact_observation: None,
                archive_ref: None,
            },
            authorization_source: CommandAuthorizationSource::ExplicitUser,
            approval_provenance: serde_json::json!({ "decision": "approved" }),
            permission_provenance: serde_json::json!({ "mode": "default" }),
            created_at: 3,
        })
        .unwrap();

    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let response = handle_request(
        storage.as_ref(),
        &agent_service,
        notifications,
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: JsonRpcId::Number(17),
            method: STORAGE_FORK_CONVERSATION_METHOD.to_string(),
            params: Some(
                serde_json::to_value(ForkConversationRequest {
                    request_id: "fork-rpc-request".to_string(),
                    source_conversation_id: "conversation-fork-rpc".to_string(),
                    fork_point: ConversationForkPoint::AssistantReply {
                        assistant_message_id: "assistant-fork-rpc".to_string(),
                    },
                })
                .unwrap(),
            ),
        },
    );

    assert_eq!(response["error"]["code"], -32000, "{response}");
    assert_eq!(
        response["error"]["message"],
        "当前任务仍有命令正在运行，请先关闭程序或等待命令结束后再继续新任务。"
    );
    assert_eq!(
        response["error"]["data"],
        serde_json::json!({
            "type": "conversation_fork",
            "code": "active_command_session",
            "conversationId": "conversation-fork-rpc",
            "activeSessionCount": 1,
        })
    );
}

#[test]
fn fork_request_rejects_retired_or_extra_fields() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
    let agent_service = AgentService::new_authorized_for_test(Arc::clone(&storage));

    for (id, params) in [
        (
            18,
            serde_json::json!({
                "requestId": "retired-fork-request",
                "sourceConversationId": "conversation-1",
                "throughAssistantMessageId": "assistant-1"
            }),
        ),
        (
            19,
            serde_json::json!({
                "requestId": "missing-fork-point",
                "sourceConversationId": "conversation-1"
            }),
        ),
        (
            20,
            serde_json::json!({
                "requestId": "extra-fork-field",
                "sourceConversationId": "conversation-1",
                "forkPoint": {
                    "kind": "assistant_reply",
                    "assistantMessageId": "assistant-1",
                    "operationId": "not-allowed"
                }
            }),
        ),
    ] {
        let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let response = handle_request(
            storage.as_ref(),
            &agent_service,
            notifications,
            JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: JsonRpcId::Number(id),
                method: STORAGE_FORK_CONVERSATION_METHOD.to_string(),
                params: Some(params),
            },
        );
        assert_eq!(response["error"]["code"], -32602, "{response}");
    }
}
