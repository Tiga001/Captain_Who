use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use mycopilot_mcp_client::{
    BoxMcpFuture, McpConnector, McpManagerPolicy, McpPeer, McpRegistry, NoopMcpEventSink,
};
use serde_json::Value;
use tempfile::TempDir;

use super::*;

mod authorization;
mod catalog;
mod lifecycle;
mod renderer_projection;
mod server_config;

struct RejectingConnector;

impl McpConnector for RejectingConnector {
    fn connect<'a>(&'a self, _: &'a McpServerConfig) -> BoxMcpFuture<'a, Arc<dyn McpPeer>> {
        Box::pin(async {
            Err(McpError::spawn(
                "the management test connector never starts a process",
            ))
        })
    }
}

struct TestHarness {
    _database_directory: TempDir,
    registry: Arc<SqliteMcpRegistry>,
    manager: Arc<McpConnectionManager>,
    service: Arc<McpManagementService>,
}

impl TestHarness {
    fn new() -> Self {
        Self::with_connector(Arc::new(RejectingConnector))
    }

    fn with_connector(connector: Arc<dyn McpConnector>) -> Self {
        let database_directory = tempfile::tempdir().expect("temporary database directory");
        let registry = Arc::new(
            SqliteMcpRegistry::open(database_directory.path().join("mcp-registry.sqlite3"))
                .expect("open test MCP Registry"),
        );
        let registry_for_manager: Arc<dyn McpRegistry> = registry.clone();
        let manager = Arc::new(
            McpConnectionManager::new(
                registry_for_manager,
                connector,
                Arc::new(NoopMcpEventSink),
                McpManagerPolicy::default(),
            )
            .expect("construct test MCP connection manager"),
        );
        let service = Arc::new(McpManagementService::new(registry.clone(), manager.clone()));
        Self {
            _database_directory: database_directory,
            registry,
            manager,
            service,
        }
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

fn create_input(display_name: &str) -> McpServerCreateInput {
    McpServerCreateInput {
        schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
        display_name: display_name.to_string(),
        transport: McpTransportKindDto::Stdio,
        executable: "/usr/bin/false".to_string(),
        arguments: vec!["--fixture".to_string(), String::new()],
        cwd: "/tmp".to_string(),
        approval_mode: McpApprovalModeDto::Prompt,
    }
}

fn mutation_input(server: &McpServerDetailsView) -> McpServerMutationInput {
    McpServerMutationInput {
        schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
        server_id: server.summary.server_id.clone(),
        precondition: precondition(server),
    }
}

fn precondition(server: &McpServerDetailsView) -> McpServerMutationPrecondition {
    McpServerMutationPrecondition {
        expected_registry_revision: server.summary.registry_revision,
        expected_config_epoch: server.summary.config_epoch.clone(),
        expected_config_digest: server.summary.config_digest.clone(),
    }
}

fn update_input(
    server: &McpServerDetailsView,
    display_name: &str,
    arguments: Vec<String>,
) -> McpServerUpdateInput {
    McpServerUpdateInput {
        schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
        server_id: server.summary.server_id.clone(),
        precondition: precondition(server),
        display_name: display_name.to_string(),
        transport: McpTransportKindDto::Stdio,
        executable: server.executable.clone(),
        arguments,
        cwd: server.cwd.clone(),
        approval_mode: server.summary.approval_mode,
    }
}

fn commit_input(preview: &McpLaunchAuthorizationPreview) -> McpLaunchAuthorizationCommitInput {
    McpLaunchAuthorizationCommitInput {
        schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
        authorization_id: preview.authorization_id.clone(),
        precondition: preview.precondition.clone(),
    }
}

fn authorize(
    service: &McpManagementService,
    server: &McpServerDetailsView,
) -> McpLaunchAuthorizationResult {
    let preview = service
        .prepare_launch_authorization(mutation_input(server))
        .expect("prepare exact launch authorization");
    service
        .commit_launch_authorization(commit_input(&preview))
        .expect("commit exact launch authorization")
}

fn server_id(server: &McpServerDetailsView) -> McpServerId {
    McpServerId::from_str(&server.summary.server_id).expect("valid server ID")
}

fn failure_code(error: McpManagementFailure) -> McpManagementErrorCodeDto {
    error.into_data().code
}

fn assert_renderer_safe_json(value: &Value) {
    const FORBIDDEN_KEYS: &[&str] = &[
        "environment",
        "env",
        "secretref",
        "token",
        "bearer",
        "header",
        "headers",
        "ciphertext",
        "nonce",
        "stderr",
        "rawarguments",
        "rawresult",
    ];
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let normalized = key.to_ascii_lowercase();
                assert!(
                    !FORBIDDEN_KEYS.contains(&normalized.as_str()),
                    "unsafe Renderer field was serialized: {key}"
                );
                assert_renderer_safe_json(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                assert_renderer_safe_json(child);
            }
        }
        _ => {}
    }
}
