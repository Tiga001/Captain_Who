use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use mycopilot_mcp_client::McpEvent;
use mycopilot_protocol_rs::{
    error_with_data, JsonRpcId, JsonRpcRequest, McpCatalogToolsPageInput,
    McpLaunchAuthorizationCommitInput, McpManagementErrorCodeDto, McpManagementErrorData,
    McpManagementErrorTypeDto, McpManagementOperationDto, McpManagementRecoveryDto,
    McpServerCreateInput, McpServerIdInput, McpServerListInput, McpServerMutationInput,
    McpServerUpdateInput, MCP_CATALOG_REFRESH_METHOD, MCP_CATALOG_TOOLS_METHOD,
    MCP_MANAGEMENT_ERROR_CODE, MCP_SERVER_ADD_METHOD, MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD,
    MCP_SERVER_AUTHORIZE_LAUNCH_PREPARE_METHOD, MCP_SERVER_DELETE_METHOD,
    MCP_SERVER_DISABLE_METHOD, MCP_SERVER_ENABLE_METHOD, MCP_SERVER_GET_METHOD,
    MCP_SERVER_LIST_METHOD, MCP_SERVER_RESTART_METHOD, MCP_SERVER_START_METHOD,
    MCP_SERVER_STATUS_METHOD, MCP_SERVER_STOP_METHOD, MCP_SERVER_UPDATE_METHOD,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{broadcast, mpsc};

use crate::application::mcp::management::{McpManagementFailure, McpManagementService};

use super::{parse_params, response_error, response_success};

pub(crate) fn is_mcp_management_method(method: &str) -> bool {
    matches!(
        method,
        MCP_SERVER_LIST_METHOD
            | MCP_SERVER_GET_METHOD
            | MCP_SERVER_ADD_METHOD
            | MCP_SERVER_UPDATE_METHOD
            | MCP_SERVER_DELETE_METHOD
            | MCP_SERVER_AUTHORIZE_LAUNCH_PREPARE_METHOD
            | MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD
            | MCP_SERVER_ENABLE_METHOD
            | MCP_SERVER_DISABLE_METHOD
            | MCP_SERVER_START_METHOD
            | MCP_SERVER_STOP_METHOD
            | MCP_SERVER_RESTART_METHOD
            | MCP_SERVER_STATUS_METHOD
            | MCP_CATALOG_TOOLS_METHOD
            | MCP_CATALOG_REFRESH_METHOD
    )
}

pub(crate) async fn handle_mcp_management_request(
    service: Arc<McpManagementService>,
    request: JsonRpcRequest,
) -> Value {
    let id = request.id;
    if request.jsonrpc != "2.0" {
        return response_error(Some(id), -32600, "Invalid JSON-RPC version");
    }
    let operation = match operation_for_method(&request.method) {
        Some(operation) => operation,
        None => return response_error(Some(id), -32601, "Method not found"),
    };
    macro_rules! input {
        ($type:ty) => {
            match parse_or_response::<$type>(&id, request.params, operation) {
                Ok(input) => input,
                Err(response) => return response,
            }
        };
    }

    match request.method.as_str() {
        MCP_SERVER_LIST_METHOD => {
            let input = input!(McpServerListInput);
            finish(id, service.list_servers(input))
        }
        MCP_SERVER_GET_METHOD | MCP_SERVER_STATUS_METHOD => {
            let input = input!(McpServerIdInput);
            finish(id, service.get_server(input, operation))
        }
        MCP_SERVER_ADD_METHOD => {
            let input = input!(McpServerCreateInput);
            finish(id, service.add_server(input))
        }
        MCP_SERVER_UPDATE_METHOD => {
            let input = input!(McpServerUpdateInput);
            finish(id, service.update_server(input).await)
        }
        MCP_SERVER_DELETE_METHOD => {
            let input = input!(McpServerMutationInput);
            finish(id, service.delete_server(input).await)
        }
        MCP_SERVER_AUTHORIZE_LAUNCH_PREPARE_METHOD => {
            let input = input!(McpServerMutationInput);
            finish(id, service.prepare_launch_authorization(input))
        }
        MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD => {
            let input = input!(McpLaunchAuthorizationCommitInput);
            finish(id, service.commit_launch_authorization(input))
        }
        MCP_SERVER_ENABLE_METHOD => {
            let input = input!(McpServerMutationInput);
            finish(id, service.enable_server(input))
        }
        MCP_SERVER_DISABLE_METHOD => {
            let input = input!(McpServerMutationInput);
            finish(id, service.disable_server(input).await)
        }
        MCP_SERVER_START_METHOD | MCP_SERVER_RESTART_METHOD => {
            let input = input!(McpServerMutationInput);
            finish(id, service.start_server(input, operation).await)
        }
        MCP_SERVER_STOP_METHOD => {
            let input = input!(McpServerMutationInput);
            finish(id, service.stop_server(input).await)
        }
        MCP_CATALOG_TOOLS_METHOD => {
            let input = input!(McpCatalogToolsPageInput);
            finish(id, service.list_tools(input))
        }
        MCP_CATALOG_REFRESH_METHOD => {
            let input = input!(McpServerMutationInput);
            finish(id, service.refresh_catalog(input).await)
        }
        _ => response_error(Some(id), -32601, "Method not found"),
    }
}

pub(crate) fn mcp_management_unavailable_response(id: JsonRpcId, method: &str) -> Value {
    mcp_error_data_response(
        id,
        McpManagementErrorData {
            schema_version: mycopilot_protocol_rs::MCP_MANAGEMENT_SCHEMA_VERSION,
            error_type: McpManagementErrorTypeDto::McpManagement,
            operation: operation_for_method(method).unwrap_or(McpManagementOperationDto::List),
            code: McpManagementErrorCodeDto::InternalSafeError,
            recovery: McpManagementRecoveryDto::Retry,
            message: "MCP management is unavailable.".to_string(),
            server_id: None,
            current_registry_revision: None,
        },
    )
}

pub(crate) async fn run_mcp_changed_notifier(
    mut events: broadcast::Receiver<McpEvent>,
    service: Arc<McpManagementService>,
    outbound: mpsc::UnboundedSender<Value>,
) {
    const DEBOUNCE: Duration = Duration::from_millis(50);
    const MAX_PENDING_SERVERS: usize = 1_024;
    let mut sequence = 0_u64;
    loop {
        let first = match events.recv().await {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                sequence = sequence.saturating_add(skipped).saturating_add(1);
                if !send_changed_notification(
                    &outbound,
                    service.project_resync_required_notification(sequence),
                ) {
                    return;
                }
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => return,
        };
        let mut pending = BTreeMap::new();
        let mut resync_required = None;
        sequence = sequence.saturating_add(1);
        if let Some(notification) = service.project_changed_notification(sequence, &first) {
            if notification.kind == mycopilot_protocol_rs::McpChangedKindDto::ResyncRequired {
                resync_required = Some(notification);
            } else if let Some(server_id) = notification.server_id.clone() {
                pending.insert(server_id, notification);
            }
        }
        let deadline = tokio::time::sleep(DEBOUNCE);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => break,
                received = events.recv() => {
                    match received {
                        Ok(event) => {
                            sequence = sequence.saturating_add(1);
                            if let Some(notification) =
                                service.project_changed_notification(sequence, &event)
                            {
                                if resync_required.is_some()
                                    || notification.kind
                                        == mycopilot_protocol_rs::McpChangedKindDto::ResyncRequired
                                {
                                    pending.clear();
                                    resync_required =
                                        Some(service.project_resync_required_notification(sequence));
                                } else if let Some(server_id) = notification.server_id.clone() {
                                    if pending.len() < MAX_PENDING_SERVERS
                                        || pending.contains_key(&server_id)
                                    {
                                        pending.insert(server_id, notification);
                                    } else {
                                        pending.clear();
                                        resync_required = Some(
                                            service.project_resync_required_notification(sequence),
                                        );
                                    }
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            sequence = sequence.saturating_add(skipped).saturating_add(1);
                            pending.clear();
                            resync_required =
                                Some(service.project_resync_required_notification(sequence));
                            break;
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        }
        // BTreeMap orders by Server ID, not event sequence. Emitting in that order can make
        // sequence numbers go backwards across servers and cause a client to discard a valid
        // invalidation. Restore causal ordering after per-server coalescing.
        let mut notifications = resync_required
            .into_iter()
            .chain(pending.into_values())
            .collect::<Vec<_>>();
        notifications.sort_by_key(|notification| notification.sequence);
        for notification in notifications {
            if !send_changed_notification(&outbound, notification) {
                return;
            }
        }
    }
}

fn send_changed_notification(
    outbound: &mpsc::UnboundedSender<Value>,
    notification: mycopilot_protocol_rs::McpChangedNotification,
) -> bool {
    outbound
        .send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": mycopilot_protocol_rs::MCP_CHANGED_NOTIFICATION_METHOD,
            "params": notification,
        }))
        .is_ok()
}

fn parse_or_response<T: DeserializeOwned>(
    id: &JsonRpcId,
    params: Option<Value>,
    operation: McpManagementOperationDto,
) -> Result<T, Value> {
    parse_params(params).map_err(|_| {
        mcp_error_data_response(
            id.clone(),
            McpManagementErrorData {
                schema_version: mycopilot_protocol_rs::MCP_MANAGEMENT_SCHEMA_VERSION,
                error_type: McpManagementErrorTypeDto::McpManagement,
                operation,
                code: McpManagementErrorCodeDto::InvalidInput,
                recovery: McpManagementRecoveryDto::FixInput,
                message: "The MCP management request is invalid.".to_string(),
                server_id: None,
                current_registry_revision: None,
            },
        )
    })
}

fn finish<T: serde::Serialize>(id: JsonRpcId, result: Result<T, McpManagementFailure>) -> Value {
    match result {
        Ok(result) => response_success(id, result),
        Err(failure) => mcp_management_error_response(id, failure),
    }
}

fn mcp_management_error_response(id: JsonRpcId, failure: McpManagementFailure) -> Value {
    mcp_error_data_response(id, failure.into_data())
}

fn mcp_error_data_response(id: JsonRpcId, data: McpManagementErrorData) -> Value {
    let message = data.message.clone();
    serde_json::to_value(error_with_data(
        Some(id),
        MCP_MANAGEMENT_ERROR_CODE,
        message,
        serde_json::to_value(data).expect("MCP management error data must serialize"),
    ))
    .expect("MCP management JSON-RPC error response must serialize")
}

fn operation_for_method(method: &str) -> Option<McpManagementOperationDto> {
    match method {
        MCP_SERVER_LIST_METHOD => Some(McpManagementOperationDto::List),
        MCP_SERVER_GET_METHOD => Some(McpManagementOperationDto::Get),
        MCP_SERVER_ADD_METHOD => Some(McpManagementOperationDto::Add),
        MCP_SERVER_UPDATE_METHOD => Some(McpManagementOperationDto::Update),
        MCP_SERVER_DELETE_METHOD => Some(McpManagementOperationDto::Delete),
        MCP_SERVER_AUTHORIZE_LAUNCH_PREPARE_METHOD => {
            Some(McpManagementOperationDto::PrepareLaunchAuthorization)
        }
        MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD => {
            Some(McpManagementOperationDto::CommitLaunchAuthorization)
        }
        MCP_SERVER_ENABLE_METHOD => Some(McpManagementOperationDto::Enable),
        MCP_SERVER_DISABLE_METHOD => Some(McpManagementOperationDto::Disable),
        MCP_SERVER_START_METHOD => Some(McpManagementOperationDto::Start),
        MCP_SERVER_STOP_METHOD => Some(McpManagementOperationDto::Stop),
        MCP_SERVER_RESTART_METHOD => Some(McpManagementOperationDto::Restart),
        MCP_SERVER_STATUS_METHOD => Some(McpManagementOperationDto::Status),
        MCP_CATALOG_TOOLS_METHOD => Some(McpManagementOperationDto::ListTools),
        MCP_CATALOG_REFRESH_METHOD => Some(McpManagementOperationDto::RefreshCatalog),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use mycopilot_mcp_client::{
        BoxMcpFuture, McpConnectionManager, McpConnector, McpError, McpManagerPolicy, McpPeer,
        McpRegistry, McpServerConfig, McpServerId, McpServerState, NoopMcpEventSink,
    };
    use mycopilot_protocol_rs::{McpTrustLevelDto, MCP_MANAGEMENT_SCHEMA_VERSION};
    use serde_json::json;

    use crate::application::mcp::sqlite_registry::SqliteMcpRegistry;

    use super::*;

    struct RejectingConnector {
        attempts: Arc<AtomicUsize>,
    }

    impl McpConnector for RejectingConnector {
        fn connect<'a>(&'a self, _: &'a McpServerConfig) -> BoxMcpFuture<'a, Arc<dyn McpPeer>> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            Box::pin(async {
                Err(McpError::spawn(
                    "the RPC test connector never starts a process",
                ))
            })
        }
    }

    struct RpcHarness {
        _database_directory: tempfile::TempDir,
        manager: Arc<McpConnectionManager>,
        service: Arc<McpManagementService>,
        connector_attempts: Arc<AtomicUsize>,
    }

    impl RpcHarness {
        fn new() -> Self {
            let database_directory = tempfile::tempdir().expect("temporary database directory");
            let registry = Arc::new(
                SqliteMcpRegistry::open(database_directory.path().join("mcp-rpc.sqlite3"))
                    .expect("open temporary MCP Registry"),
            );
            let connector_attempts = Arc::new(AtomicUsize::new(0));
            let connector = Arc::new(RejectingConnector {
                attempts: Arc::clone(&connector_attempts),
            });
            let registry_for_manager: Arc<dyn McpRegistry> = registry.clone();
            let manager = Arc::new(
                McpConnectionManager::new(
                    registry_for_manager,
                    connector,
                    Arc::new(NoopMcpEventSink),
                    McpManagerPolicy::default(),
                )
                .expect("construct test MCP Manager"),
            );
            let service = Arc::new(McpManagementService::new(registry, Arc::clone(&manager)));
            Self {
                _database_directory: database_directory,
                manager,
                service,
                connector_attempts,
            }
        }

        async fn request(&self, id: i64, method: &str, params: Value) -> Value {
            handle_mcp_management_request(
                Arc::clone(&self.service),
                JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    id: JsonRpcId::Number(id),
                    method: method.to_string(),
                    params: Some(params),
                },
            )
            .await
        }

        async fn shutdown(&self) {
            self.service.begin_shutdown();
            let report = self.manager.shutdown(Duration::from_millis(100)).await;
            assert!(
                report.cleanup_complete,
                "test MCP Manager cleanup must complete: {report:?}"
            );
        }
    }

    fn create_params(display_name: &str) -> Value {
        json!({
            "schemaVersion": MCP_MANAGEMENT_SCHEMA_VERSION,
            "displayName": display_name,
            "transport": "stdio",
            "executable": "/usr/bin/false",
            "arguments": ["--fixture", ""],
            "cwd": "/tmp",
            "approvalMode": "prompt",
        })
    }

    fn mutation_params(server: &Value) -> Value {
        json!({
            "schemaVersion": MCP_MANAGEMENT_SCHEMA_VERSION,
            "serverId": server["serverId"],
            "precondition": {
                "expectedRegistryRevision": server["registryRevision"],
                "expectedConfigEpoch": server["configEpoch"],
                "expectedConfigDigest": server["configDigest"],
            },
        })
    }

    fn assert_management_error(response: &Value, operation: &str, code: &str) {
        assert_eq!(response["error"]["code"], MCP_MANAGEMENT_ERROR_CODE);
        assert_eq!(
            response["error"]["data"]["schemaVersion"],
            MCP_MANAGEMENT_SCHEMA_VERSION
        );
        assert_eq!(response["error"]["data"]["type"], "mcpManagement");
        assert_eq!(response["error"]["data"]["operation"], operation);
        assert_eq!(response["error"]["data"]["code"], code);
        assert!(response["error"]["data"]["message"]
            .as_str()
            .is_some_and(|message| message.len() <= 4096));
    }

    #[tokio::test]
    async fn rpc_add_list_and_get_return_safe_persisted_views_without_connecting() {
        let harness = RpcHarness::new();
        let added = harness
            .request(1, MCP_SERVER_ADD_METHOD, create_params("RPC fixture"))
            .await;
        let server = &added["result"]["server"];
        assert!(server["serverId"].as_str().is_some());
        assert_eq!(server["displayName"], "RPC fixture");
        assert_eq!(server["enabled"], false);
        assert_eq!(server["trust"], "untrusted");
        assert_eq!(server["approvalMode"], "prompt");
        assert_eq!(server["launchAuthorizationState"], "required");

        let listed = harness
            .request(
                2,
                MCP_SERVER_LIST_METHOD,
                json!({ "schemaVersion": MCP_MANAGEMENT_SCHEMA_VERSION }),
            )
            .await;
        assert_eq!(
            listed["result"]["servers"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(
            listed["result"]["servers"][0]["serverId"],
            server["serverId"]
        );

        let fetched = harness
            .request(
                3,
                MCP_SERVER_GET_METHOD,
                json!({
                    "schemaVersion": MCP_MANAGEMENT_SCHEMA_VERSION,
                    "serverId": server["serverId"],
                }),
            )
            .await;
        assert_eq!(fetched["result"]["server"]["serverId"], server["serverId"]);
        assert_eq!(fetched["result"]["server"]["arguments"][1], "");
        assert_eq!(harness.connector_attempts.load(Ordering::Relaxed), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn rpc_launch_authorization_is_one_shot_and_enable_is_a_separate_cas_mutation() {
        let harness = RpcHarness::new();
        let added = harness
            .request(
                1,
                MCP_SERVER_ADD_METHOD,
                create_params("authorization fixture"),
            )
            .await;
        let added_server = &added["result"]["server"];

        let denied = harness
            .request(2, MCP_SERVER_ENABLE_METHOD, mutation_params(added_server))
            .await;
        assert_management_error(&denied, "enable", "authorizationRequired");

        let prepared = harness
            .request(
                3,
                MCP_SERVER_AUTHORIZE_LAUNCH_PREPARE_METHOD,
                mutation_params(added_server),
            )
            .await;
        let preview = &prepared["result"];
        assert_eq!(preview["serverId"], added_server["serverId"]);
        assert_eq!(preview["arguments"][1], "");
        let commit_params = json!({
            "schemaVersion": MCP_MANAGEMENT_SCHEMA_VERSION,
            "authorizationId": preview["authorizationId"],
            "precondition": preview["precondition"],
        });
        let committed = harness
            .request(
                4,
                MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD,
                commit_params.clone(),
            )
            .await;
        let authorized_server = &committed["result"]["server"];
        assert_eq!(committed["result"]["authorized"], true);
        assert_eq!(authorized_server["trust"], "userApproved");
        assert_eq!(authorized_server["enabled"], false);

        let replay = harness
            .request(5, MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD, commit_params)
            .await;
        assert_management_error(&replay, "commitLaunchAuthorization", "authorizationStale");

        let enabled = harness
            .request(
                6,
                MCP_SERVER_ENABLE_METHOD,
                mutation_params(authorized_server),
            )
            .await;
        assert_eq!(enabled["result"]["server"]["enabled"], true);
        assert_eq!(
            enabled["result"]["server"]["trust"],
            serde_json::to_value(McpTrustLevelDto::UserApproved)
                .expect("serialize expected trust enum")
        );
        assert_eq!(harness.connector_attempts.load(Ordering::Relaxed), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn rpc_rejects_unknown_fields_wrong_casing_version_and_enum_without_echoing_input() {
        let harness = RpcHarness::new();
        let canary = "MCP_RPC_INPUT_CANARY_MUST_NOT_ECHO";

        let mut unknown_field = create_params("strict fixture");
        unknown_field
            .as_object_mut()
            .expect("create params object")
            .insert("environment".to_string(), json!({ "value": canary }));
        let unknown = harness
            .request(1, MCP_SERVER_ADD_METHOD, unknown_field)
            .await;
        assert_management_error(&unknown, "add", "invalidInput");
        assert!(!unknown.to_string().contains(canary));

        let wrong_casing = harness
            .request(
                2,
                MCP_SERVER_LIST_METHOD,
                json!({ "schema_version": MCP_MANAGEMENT_SCHEMA_VERSION }),
            )
            .await;
        assert_management_error(&wrong_casing, "list", "invalidInput");

        let wrong_version = harness
            .request(
                3,
                MCP_SERVER_LIST_METHOD,
                json!({ "schemaVersion": MCP_MANAGEMENT_SCHEMA_VERSION + 1 }),
            )
            .await;
        assert_management_error(&wrong_version, "list", "invalidInput");

        let mut unknown_transport = create_params("invalid transport");
        unknown_transport["transport"] = Value::String("streamableHttp".to_string());
        let transport = harness
            .request(4, MCP_SERVER_ADD_METHOD, unknown_transport)
            .await;
        assert_management_error(&transport, "add", "invalidInput");
        assert_eq!(harness.connector_attempts.load(Ordering::Relaxed), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn rpc_not_found_and_unavailable_fail_with_structured_safe_errors() {
        let harness = RpcHarness::new();
        let missing_id = McpServerId::new().to_string();
        let missing = harness
            .request(
                1,
                MCP_SERVER_GET_METHOD,
                json!({
                    "schemaVersion": MCP_MANAGEMENT_SCHEMA_VERSION,
                    "serverId": missing_id,
                }),
            )
            .await;
        assert_management_error(&missing, "get", "notFound");
        assert_eq!(
            missing["error"]["data"]["serverId"].as_str(),
            Some(missing_id.as_str())
        );

        let unavailable =
            mcp_management_unavailable_response(JsonRpcId::Number(2), MCP_SERVER_ENABLE_METHOD);
        assert_management_error(&unavailable, "enable", "internalSafeError");
        assert_eq!(unavailable["error"]["data"]["recovery"], "retry");

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn changed_notifications_preserve_monotonic_sequence_after_server_coalescing() {
        let harness = RpcHarness::new();
        let high_id =
            McpServerId::from_str("ffffffff-ffff-4fff-bfff-ffffffffffff").expect("valid high UUID");
        let low_id =
            McpServerId::from_str("00000000-0000-4000-8000-000000000001").expect("valid low UUID");
        let (event_sender, event_receiver) = broadcast::channel(8);
        let (outbound, mut notifications) = mpsc::unbounded_channel();
        let notifier = tokio::spawn(run_mcp_changed_notifier(
            event_receiver,
            Arc::clone(&harness.service),
            outbound,
        ));

        event_sender
            .send(McpEvent::ServerStateChanged {
                server_id: high_id,
                sequence: 1,
                previous: McpServerState::Disabled,
                current: McpServerState::Starting,
            })
            .expect("send first state event");
        event_sender
            .send(McpEvent::ServerStateChanged {
                server_id: high_id,
                sequence: 2,
                previous: McpServerState::Starting,
                current: McpServerState::Ready,
            })
            .expect("send coalesced state event");
        event_sender
            .send(McpEvent::ServerStateChanged {
                server_id: low_id,
                sequence: 3,
                previous: McpServerState::Disabled,
                current: McpServerState::Ready,
            })
            .expect("send final state event");

        let first = tokio::time::timeout(Duration::from_secs(1), notifications.recv())
            .await
            .expect("first notification timeout")
            .expect("first notification");
        let second = tokio::time::timeout(Duration::from_secs(1), notifications.recv())
            .await
            .expect("second notification timeout")
            .expect("second notification");
        assert!(first["params"]["sequence"].as_u64() < second["params"]["sequence"].as_u64());
        assert_eq!(first["params"]["serverId"], high_id.to_string());
        assert_eq!(second["params"]["serverId"], low_id.to_string());
        assert_eq!(first["params"]["state"], "ready");
        assert_eq!(second["params"]["state"], "ready");
        let source_epoch = first["params"]["sourceEpoch"]
            .as_str()
            .expect("first source epoch");
        assert_eq!(second["params"]["sourceEpoch"].as_str(), Some(source_epoch));
        assert_eq!(
            uuid::Uuid::parse_str(source_epoch)
                .expect("source epoch UUID")
                .get_version_num(),
            4
        );

        notifier.abort();
        let _ = notifier.await;
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn changed_notification_broadcast_lag_emits_global_resync_without_fake_server() {
        let harness = RpcHarness::new();
        let server_id = McpServerId::new();
        let (event_sender, event_receiver) = broadcast::channel(1);
        event_sender
            .send(McpEvent::ServerStateChanged {
                server_id,
                sequence: 1,
                previous: McpServerState::Disabled,
                current: McpServerState::Starting,
            })
            .expect("send event that will be skipped");
        event_sender
            .send(McpEvent::ServerStateChanged {
                server_id,
                sequence: 2,
                previous: McpServerState::Starting,
                current: McpServerState::Ready,
            })
            .expect("send retained event");
        let (outbound, mut notifications) = mpsc::unbounded_channel();
        let notifier = tokio::spawn(run_mcp_changed_notifier(
            event_receiver,
            Arc::clone(&harness.service),
            outbound,
        ));

        let notification = tokio::time::timeout(Duration::from_secs(1), notifications.recv())
            .await
            .expect("resync notification timeout")
            .expect("resync notification");
        assert_eq!(notification["params"]["kind"], "resyncRequired");
        assert!(notification["params"].get("serverId").is_none());
        assert!(notification["params"].get("state").is_none());
        assert_eq!(
            uuid::Uuid::parse_str(
                notification["params"]["sourceEpoch"]
                    .as_str()
                    .expect("source epoch")
            )
            .expect("source epoch UUID")
            .get_version_num(),
            4
        );

        notifier.abort();
        let _ = notifier.await;
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn changed_notification_pending_overflow_collapses_to_one_global_resync() {
        let harness = RpcHarness::new();
        let (event_sender, event_receiver) = broadcast::channel(2_048);
        let (outbound, mut notifications) = mpsc::unbounded_channel();
        let notifier = tokio::spawn(run_mcp_changed_notifier(
            event_receiver,
            Arc::clone(&harness.service),
            outbound,
        ));

        for index in 1..=1_025_u64 {
            let server_id = McpServerId::from_str(&format!("00000000-0000-4000-8000-{index:012}"))
                .expect("bounded unique Server ID");
            event_sender
                .send(McpEvent::ServerStateChanged {
                    server_id,
                    sequence: index,
                    previous: McpServerState::Disabled,
                    current: McpServerState::Ready,
                })
                .expect("send state event");
        }

        let notification = tokio::time::timeout(Duration::from_secs(1), notifications.recv())
            .await
            .expect("overflow resync timeout")
            .expect("overflow resync notification");
        assert_eq!(notification["params"]["kind"], "resyncRequired");
        assert_eq!(notification["params"]["sequence"], 1_025);
        assert!(notification["params"].get("serverId").is_none());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), notifications.recv())
                .await
                .is_err(),
            "overflow must not forward a partial per-Server batch"
        );

        notifier.abort();
        let _ = notifier.await;
        harness.shutdown().await;
    }
}
