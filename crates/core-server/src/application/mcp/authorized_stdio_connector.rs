use std::collections::BTreeSet;
use std::sync::Arc;

use mycopilot_mcp_client::{
    config_digest, BoxMcpFuture, McpConnector, McpDispatchCertainty, McpError, McpPeer,
    McpServerConfig, McpServerScope, McpStdioConnector, McpStdioPolicy, McpTransportConfig,
    McpTrustLevel,
};

use super::sqlite_registry::{
    compute_launch_spec_digest, McpPersistedRegistryRecord, McpRegistryPersistenceError,
    McpServerSource, SqliteMcpRegistry, MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION,
    MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
};

/// Host-owned connector that turns a durable, exact logical launch authorization into
/// a single-use stdio connector policy immediately before process creation.
///
/// The Connection Manager supplies only an `McpServerConfig`, so this boundary
/// re-reads the durable Registry on every attempt. A stale in-memory config,
/// an authorization for another Server, or a changed persisted path/argv/cwd
/// specification is rejected before the stdio connector can spawn. This does
/// not bind the executable's filesystem object identity; descriptor-bound
/// process launch remains a separate platform-hardening requirement.
pub(crate) struct AuthorizedMcpStdioConnector {
    registry: Arc<SqliteMcpRegistry>,
    policy_template: McpStdioPolicy,
}

impl AuthorizedMcpStdioConnector {
    pub(crate) fn new(registry: Arc<SqliteMcpRegistry>) -> Self {
        Self::with_policy(registry, McpStdioPolicy::default())
    }

    pub(crate) fn with_policy(
        registry: Arc<SqliteMcpRegistry>,
        mut policy_template: McpStdioPolicy,
    ) -> Self {
        // Allow lists are derived from the exact authorized record for every
        // connect. A caller cannot smuggle broader process or environment
        // authority through the reusable policy template.
        policy_template.allowed_programs.clear();
        policy_template.allowed_host_variables.clear();
        Self {
            registry,
            policy_template,
        }
    }

    fn prepare_authorized_launch(
        &self,
        requested: &McpServerConfig,
    ) -> Result<AuthorizedStdioLaunch, McpError> {
        // This is intentionally the final durable authority read before the
        // concrete connector is built. There is no await between this read and
        // the delegate's spawn path.
        let persisted = self
            .registry
            .get_persisted(requested.id)
            .map_err(map_registry_read_error)?
            .ok_or_else(|| authorization_error("MCP stdio launch is not authorized"))?;

        validate_persisted_authorization(requested, &persisted)?;

        let McpTransportConfig::Stdio(stdio) = &persisted.entry.config.transport else {
            return Err(authorization_error(
                "MCP stdio launch transport is not authorized",
            ));
        };
        let mut policy = self.policy_template.clone();
        policy.allowed_programs = BTreeSet::from([stdio.program.clone()]);
        policy.allowed_host_variables.clear();

        Ok(AuthorizedStdioLaunch {
            config: persisted.entry.config,
            policy,
        })
    }
}

impl McpConnector for AuthorizedMcpStdioConnector {
    fn connect<'a>(&'a self, config: &'a McpServerConfig) -> BoxMcpFuture<'a, Arc<dyn McpPeer>> {
        Box::pin(async move {
            let launch = self.prepare_authorized_launch(config)?;
            let connector = McpStdioConnector::new(launch.policy);
            let peer: Arc<dyn McpPeer> = connector.connect(&launch.config).await?;
            Ok(peer)
        })
    }
}

struct AuthorizedStdioLaunch {
    config: McpServerConfig,
    policy: McpStdioPolicy,
}

fn validate_persisted_authorization(
    requested: &McpServerConfig,
    persisted: &McpPersistedRegistryRecord,
) -> Result<(), McpError> {
    let requested_digest = config_digest(requested)
        .map_err(|_| authorization_error("MCP stdio launch configuration is invalid"))?;
    if persisted.entry.config.id != requested.id
        || persisted.entry.config_digest != requested_digest
        || persisted.entry.config != *requested
    {
        return Err(authorization_error(
            "MCP stdio launch configuration is stale",
        ));
    }

    if persisted.source != McpServerSource::UserManual
        || persisted.entry.config.scope != McpServerScope::User
        || persisted.entry.config.trust != McpTrustLevel::UserApproved
        || !persisted.entry.config.enabled
    {
        return Err(authorization_error(
            "MCP stdio launch is outside the authorized User policy",
        ));
    }

    let McpTransportConfig::Stdio(stdio) = &persisted.entry.config.transport else {
        return Err(authorization_error(
            "MCP stdio launch transport is not authorized",
        ));
    };
    if !stdio.program.is_absolute() || !stdio.cwd.is_absolute() {
        return Err(authorization_error(
            "MCP stdio launch paths are not authorized",
        ));
    }
    if !stdio.environment.is_empty() {
        return Err(authorization_error(
            "MCP stdio launch environment is not authorized",
        ));
    }

    let computed_launch_digest = compute_launch_spec_digest(&persisted.entry.config)
        .map_err(map_registry_validation_error)?;
    if computed_launch_digest != persisted.launch_spec_digest {
        return Err(authorization_error(
            "MCP stdio launch specification is stale",
        ));
    }

    let authorization = persisted
        .launch_authorization
        .as_ref()
        .ok_or_else(|| authorization_error("MCP stdio launch is not authorized"))?;
    if authorization.authorization_format_version != MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION
        || authorization.server_id != persisted.entry.config.id
        || authorization.launch_spec_digest != computed_launch_digest
        || authorization.authored_config_epoch != persisted.entry.config_epoch
        || authorization.authored_config_digest != persisted.entry.config_digest
        || authorization.authorization_policy_version != MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION
    {
        return Err(authorization_error(
            "MCP stdio launch authorization is stale",
        ));
    }

    Ok(())
}

fn authorization_error(message: &'static str) -> McpError {
    McpError::config(message).with_dispatch_certainty(McpDispatchCertainty::DefinitelyNotDispatched)
}

fn map_registry_read_error(error: McpRegistryPersistenceError) -> McpError {
    match error {
        McpRegistryPersistenceError::StorageUnavailable
        | McpRegistryPersistenceError::RevisionExhausted => {
            McpError::protocol("MCP launch authorization storage is unavailable")
        }
        _ => authorization_error("MCP stdio launch authorization could not be verified"),
    }
}

fn map_registry_validation_error(_: McpRegistryPersistenceError) -> McpError {
    authorization_error("MCP stdio launch authorization could not be verified")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mycopilot_mcp_client::{
        McpApprovalMode, McpEnvBinding, McpErrorKind, McpRegistry, McpServerId, McpStdioConfig,
    };
    use tempfile::tempdir;

    use super::*;
    use crate::application::mcp::sqlite_registry::{
        McpRegistryMutationPrecondition, SqliteMcpRegistry,
    };

    const PRIVATE_ARGUMENT_CANARY: &str = "MCP_PRIVATE_ARGUMENT_CANARY";

    fn config(id: McpServerId, executable: PathBuf, cwd: PathBuf) -> McpServerConfig {
        McpServerConfig {
            id,
            display_name: "owned test server".to_string(),
            scope: McpServerScope::User,
            trust: McpTrustLevel::Untrusted,
            approval_mode: McpApprovalMode::Prompt,
            enabled: false,
            transport: McpTransportConfig::Stdio(McpStdioConfig {
                program: executable,
                arguments: vec!["--fixed".to_string()],
                cwd,
                environment: Vec::new(),
            }),
            connect_timeout_ms: 1_000,
            request_timeout_ms: 60_000,
            shutdown_timeout_ms: 2_000,
        }
    }

    fn authorize_and_enable(
        registry: &SqliteMcpRegistry,
        config: McpServerConfig,
        policy_version: u32,
    ) -> McpServerConfig {
        let entry = registry.add(config).unwrap();
        let persisted = registry.get_persisted(entry.config.id).unwrap().unwrap();
        registry
            .authorize_launch(
                &McpRegistryMutationPrecondition::from_entry(&persisted.entry),
                &persisted.launch_spec_digest,
                policy_version,
                1,
            )
            .unwrap();
        let authorized = registry.get_persisted(entry.config.id).unwrap().unwrap();
        registry
            .set_enabled(
                &McpRegistryMutationPrecondition::from_entry(&authorized.entry),
                true,
            )
            .unwrap();
        registry
            .get_persisted(entry.config.id)
            .unwrap()
            .unwrap()
            .entry
            .config
    }

    fn expect_mcp_error<T>(result: Result<T, McpError>) -> McpError {
        match result {
            Ok(_) => panic!("operation unexpectedly succeeded"),
            Err(error) => error,
        }
    }

    #[test]
    fn exact_authorization_builds_a_single_program_zero_environment_policy() {
        let directory = tempdir().unwrap();
        let registry =
            Arc::new(SqliteMcpRegistry::open(directory.path().join("registry.sqlite")).unwrap());
        let executable = directory.path().join("owned-fixture");
        let authorized = authorize_and_enable(
            &registry,
            config(
                McpServerId::new(),
                executable.clone(),
                directory.path().to_path_buf(),
            ),
            MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
        );

        let mut broad_template = McpStdioPolicy::default();
        broad_template
            .allowed_programs
            .insert(directory.path().join("must-not-survive"));
        broad_template
            .allowed_host_variables
            .insert("MUST_NOT_SURVIVE".to_string());
        let connector =
            AuthorizedMcpStdioConnector::with_policy(Arc::clone(&registry), broad_template);
        let launch = connector.prepare_authorized_launch(&authorized).unwrap();

        assert_eq!(launch.policy.allowed_programs, BTreeSet::from([executable]));
        assert!(launch.policy.allowed_host_variables.is_empty());
        assert_eq!(launch.config, authorized);
    }

    #[tokio::test]
    async fn missing_authorization_is_rejected_before_spawn() {
        let directory = tempdir().unwrap();
        let registry =
            Arc::new(SqliteMcpRegistry::open(directory.path().join("registry.sqlite")).unwrap());
        let server = config(
            McpServerId::new(),
            directory.path().join("does-not-exist"),
            directory.path().to_path_buf(),
        );
        registry.add(server.clone()).unwrap();
        let connector = AuthorizedMcpStdioConnector::new(registry);

        let error = expect_mcp_error(McpConnector::connect(&connector, &server).await);
        assert_eq!(error.kind, McpErrorKind::Config);
        assert_eq!(
            error.dispatch_certainty,
            Some(McpDispatchCertainty::DefinitelyNotDispatched)
        );
        assert!(!error.to_string().contains(PRIVATE_ARGUMENT_CANARY));
    }

    #[tokio::test]
    async fn caller_config_drift_is_rejected_before_spawn_without_echoing_arguments() {
        let directory = tempdir().unwrap();
        let registry =
            Arc::new(SqliteMcpRegistry::open(directory.path().join("registry.sqlite")).unwrap());
        let mut authorized = authorize_and_enable(
            &registry,
            config(
                McpServerId::new(),
                directory.path().join("does-not-exist"),
                directory.path().to_path_buf(),
            ),
            MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
        );
        let McpTransportConfig::Stdio(stdio) = &mut authorized.transport else {
            panic!("test config must use stdio");
        };
        stdio.arguments.push(PRIVATE_ARGUMENT_CANARY.to_string());
        let connector = AuthorizedMcpStdioConnector::new(registry);

        let error = expect_mcp_error(McpConnector::connect(&connector, &authorized).await);
        assert_eq!(error.kind, McpErrorKind::Config);
        assert_eq!(error.message, "MCP stdio launch configuration is stale");
        assert!(!error.to_string().contains(PRIVATE_ARGUMENT_CANARY));
    }

    #[tokio::test]
    async fn authorization_for_a_different_server_id_is_never_reused() {
        let directory = tempdir().unwrap();
        let registry =
            Arc::new(SqliteMcpRegistry::open(directory.path().join("registry.sqlite")).unwrap());
        let authorized = authorize_and_enable(
            &registry,
            config(
                McpServerId::new(),
                directory.path().join("does-not-exist"),
                directory.path().to_path_buf(),
            ),
            MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
        );
        let mut other_server = authorized;
        other_server.id = McpServerId::new();
        let connector = AuthorizedMcpStdioConnector::new(registry);

        let error = expect_mcp_error(McpConnector::connect(&connector, &other_server).await);
        assert_eq!(error.kind, McpErrorKind::Config);
        assert_eq!(error.message, "MCP stdio launch is not authorized");
    }

    #[tokio::test]
    async fn every_connect_re_reads_the_registry_and_observes_revocation() {
        let directory = tempdir().unwrap();
        let registry =
            Arc::new(SqliteMcpRegistry::open(directory.path().join("registry.sqlite")).unwrap());
        let authorized = authorize_and_enable(
            &registry,
            config(
                McpServerId::new(),
                directory.path().join("does-not-exist"),
                directory.path().to_path_buf(),
            ),
            MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
        );
        let connector = AuthorizedMcpStdioConnector::new(Arc::clone(&registry));

        let current = registry
            .get_persisted(authorized.id)
            .unwrap()
            .expect("authorized record");
        registry
            .set_enabled(
                &McpRegistryMutationPrecondition::from_entry(&current.entry),
                false,
            )
            .unwrap();

        let error = expect_mcp_error(McpConnector::connect(&connector, &authorized).await);
        assert_eq!(error.kind, McpErrorKind::Config);
        assert_eq!(
            error.dispatch_certainty,
            Some(McpDispatchCertainty::DefinitelyNotDispatched)
        );
    }

    #[test]
    fn stale_authorization_policy_is_rejected_without_process_creation() {
        let directory = tempdir().unwrap();
        let registry =
            Arc::new(SqliteMcpRegistry::open(directory.path().join("registry.sqlite")).unwrap());
        let authorized = authorize_and_enable(
            &registry,
            config(
                McpServerId::new(),
                directory.path().join("does-not-exist"),
                directory.path().to_path_buf(),
            ),
            MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
        );
        let connection = rusqlite::Connection::open(directory.path().join("registry.sqlite"))
            .expect("open test registry");
        connection
            .execute(
                "UPDATE mcp_registry_servers
                 SET authorization_policy_version = ?2
                 WHERE server_id = ?1",
                rusqlite::params![
                    authorized.id.to_string(),
                    i64::from(MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION + 1)
                ],
            )
            .expect("make the persisted policy stale");
        let connector = AuthorizedMcpStdioConnector::new(registry);

        let error = expect_mcp_error(connector.prepare_authorized_launch(&authorized));
        assert_eq!(error.kind, McpErrorKind::Config);
        assert_eq!(
            error.message,
            "MCP stdio launch authorization could not be verified"
        );
    }

    #[test]
    fn environment_binding_is_rejected_even_if_a_corrupt_record_reaches_validation() {
        let directory = tempdir().unwrap();
        let registry =
            Arc::new(SqliteMcpRegistry::open(directory.path().join("registry.sqlite")).unwrap());
        let authorized = authorize_and_enable(
            &registry,
            config(
                McpServerId::new(),
                directory.path().join("does-not-exist"),
                directory.path().to_path_buf(),
            ),
            MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
        );
        let mut persisted = registry.get_persisted(authorized.id).unwrap().unwrap();
        let McpTransportConfig::Stdio(stdio) = &mut persisted.entry.config.transport else {
            panic!("test config must use stdio");
        };
        stdio.environment.push(McpEnvBinding::Plain {
            name: "CANARY".to_string(),
            value: PRIVATE_ARGUMENT_CANARY.to_string(),
        });
        persisted.entry.config_digest = config_digest(&persisted.entry.config).unwrap();

        let error =
            validate_persisted_authorization(&persisted.entry.config, &persisted).unwrap_err();
        assert_eq!(
            error.message,
            "MCP stdio launch environment is not authorized"
        );
        assert!(!error.to_string().contains(PRIVATE_ARGUMENT_CANARY));
    }
}
