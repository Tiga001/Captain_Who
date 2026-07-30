//! Server configuration CRUD and registry precondition handling.

use super::*;

impl McpManagementService {
    pub(crate) fn list_servers(
        &self,
        input: McpServerListInput,
    ) -> Result<McpServerListOutput, McpManagementFailure> {
        ensure_schema_version(input.schema_version, McpManagementOperationDto::List)?;
        let (registry_revision, records) = self
            .registry
            .snapshot()
            .map_err(|error| self.registry_failure(McpManagementOperationDto::List, None, error))?;
        let mut servers = records
            .iter()
            .map(|record| self.project_summary(record))
            .collect::<Result<Vec<_>, _>>()?;
        servers.sort_by(|left, right| left.server_id.cmp(&right.server_id));
        Ok(McpServerListOutput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            registry_revision,
            servers,
        })
    }

    pub(crate) fn get_server(
        &self,
        input: McpServerIdInput,
        operation: McpManagementOperationDto,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        ensure_schema_version(input.schema_version, operation)?;
        let server_id = parse_server_id(&input.server_id, operation)?;
        self.details_for(server_id, operation)
    }

    pub(crate) fn add_server(
        &self,
        input: McpServerCreateInput,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::Add)?;
        ensure_schema_version(input.schema_version, McpManagementOperationDto::Add)?;
        validate_editable_fields(
            &input.display_name,
            &input.executable,
            &input.arguments,
            &input.cwd,
            McpManagementOperationDto::Add,
        )?;
        let config = McpServerConfig {
            id: McpServerId::new(),
            display_name: input.display_name,
            scope: McpServerScope::User,
            trust: McpTrustLevel::Untrusted,
            approval_mode: approval_mode(input.approval_mode),
            enabled: false,
            transport: McpTransportConfig::Stdio(McpStdioConfig {
                program: input.executable.into(),
                arguments: input.arguments,
                cwd: input.cwd.into(),
                environment: Vec::new(),
            }),
            connect_timeout_ms: McpServerConfig::default_connect_timeout_ms(),
            request_timeout_ms: McpServerConfig::default_request_timeout_ms(),
            shutdown_timeout_ms: McpServerConfig::default_shutdown_timeout_ms(),
        };
        let entry = self
            .registry
            .add_persisted(config)
            .map_err(|error| self.registry_failure(McpManagementOperationDto::Add, None, error))?;
        self.details_for(entry.config.id, McpManagementOperationDto::Add)
    }

    pub(crate) async fn update_server(
        &self,
        input: McpServerUpdateInput,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::Update)?;
        ensure_schema_version(input.schema_version, McpManagementOperationDto::Update)?;
        validate_editable_fields(
            &input.display_name,
            &input.executable,
            &input.arguments,
            &input.cwd,
            McpManagementOperationDto::Update,
        )?;
        let precondition = parse_precondition(
            &input.server_id,
            &input.precondition,
            McpManagementOperationDto::Update,
        )?;
        let _active_mutation =
            self.begin_server_mutation(precondition.server_id, McpManagementOperationDto::Update)?;
        let existing = self.persisted(precondition.server_id, McpManagementOperationDto::Update)?;
        ensure_registry_precondition(&existing.entry, &precondition).map_err(|_| {
            self.registry_failure(
                McpManagementOperationDto::Update,
                Some(precondition.server_id),
                McpRegistryPersistenceError::Conflict,
            )
        })?;
        let config = McpServerConfig {
            id: precondition.server_id,
            display_name: input.display_name,
            scope: McpServerScope::User,
            trust: existing.entry.config.trust,
            approval_mode: approval_mode(input.approval_mode),
            enabled: existing.entry.config.enabled,
            transport: McpTransportConfig::Stdio(McpStdioConfig {
                program: input.executable.into(),
                arguments: input.arguments,
                cwd: input.cwd.into(),
                environment: Vec::new(),
            }),
            connect_timeout_ms: existing.entry.config.connect_timeout_ms,
            request_timeout_ms: existing.entry.config.request_timeout_ms,
            shutdown_timeout_ms: existing.entry.config.shutdown_timeout_ms,
        };
        let proposed_launch_digest = compute_launch_spec_digest(&config).map_err(|error| {
            self.registry_failure(
                McpManagementOperationDto::Update,
                Some(precondition.server_id),
                error,
            )
        })?;
        let launch_changed = proposed_launch_digest != existing.launch_spec_digest;
        let mutation = self
            .registry
            .update_with_precondition(&precondition, config)
            .map_err(|error| {
                self.registry_failure(
                    McpManagementOperationDto::Update,
                    Some(precondition.server_id),
                    error,
                )
            })?;
        if launch_changed && !matches!(mutation, McpRegistryMutation::Unchanged(_)) {
            self.manager
                .stop(precondition.server_id)
                .await
                .map_err(|error| {
                    self.manager_failure(
                        McpManagementOperationDto::Update,
                        Some(precondition.server_id),
                        error,
                    )
                })?;
        }
        self.details_for(precondition.server_id, McpManagementOperationDto::Update)
    }

    pub(crate) fn enable_server(
        &self,
        input: McpServerMutationInput,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::Enable)?;
        ensure_schema_version(input.schema_version, McpManagementOperationDto::Enable)?;
        let precondition = parse_precondition(
            &input.server_id,
            &input.precondition,
            McpManagementOperationDto::Enable,
        )?;
        let _active_mutation =
            self.begin_server_mutation(precondition.server_id, McpManagementOperationDto::Enable)?;
        self.set_enabled_with_precondition(&precondition, true, McpManagementOperationDto::Enable)
    }

    pub(crate) async fn disable_server(
        &self,
        input: McpServerMutationInput,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::Disable)?;
        ensure_schema_version(input.schema_version, McpManagementOperationDto::Disable)?;
        let precondition = parse_precondition(
            &input.server_id,
            &input.precondition,
            McpManagementOperationDto::Disable,
        )?;
        let _active_mutation =
            self.begin_server_mutation(precondition.server_id, McpManagementOperationDto::Disable)?;
        let output = self.set_enabled_with_precondition(
            &precondition,
            false,
            McpManagementOperationDto::Disable,
        )?;
        let server_id = parse_server_id(
            &output.server.summary.server_id,
            McpManagementOperationDto::Disable,
        )?;
        self.manager.stop(server_id).await.map_err(|error| {
            self.manager_failure(McpManagementOperationDto::Disable, Some(server_id), error)
        })?;
        self.details_for(server_id, McpManagementOperationDto::Disable)
    }

    pub(crate) async fn delete_server(
        &self,
        input: McpServerMutationInput,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::Delete)?;
        ensure_schema_version(input.schema_version, McpManagementOperationDto::Delete)?;
        let mut precondition = parse_precondition(
            &input.server_id,
            &input.precondition,
            McpManagementOperationDto::Delete,
        )?;
        let _active_mutation =
            self.begin_server_mutation(precondition.server_id, McpManagementOperationDto::Delete)?;
        let existing = self.persisted(precondition.server_id, McpManagementOperationDto::Delete)?;
        ensure_registry_precondition(&existing.entry, &precondition).map_err(|_| {
            self.registry_failure(
                McpManagementOperationDto::Delete,
                Some(precondition.server_id),
                McpRegistryPersistenceError::Conflict,
            )
        })?;
        if existing.entry.config.enabled {
            let disabled = self
                .registry
                .set_enabled(&precondition, false)
                .map_err(|error| {
                    self.registry_failure(
                        McpManagementOperationDto::Delete,
                        Some(precondition.server_id),
                        error,
                    )
                })?;
            precondition = McpRegistryMutationPrecondition::from_entry(&disabled);
        }
        self.manager
            .stop(precondition.server_id)
            .await
            .map_err(|error| {
                self.manager_failure(
                    McpManagementOperationDto::Delete,
                    Some(precondition.server_id),
                    error,
                )
            })?;
        let before_delete =
            self.details_for(precondition.server_id, McpManagementOperationDto::Delete)?;
        let removed = self
            .registry
            .remove_with_precondition(&precondition)
            .map_err(|error| {
                self.registry_failure(
                    McpManagementOperationDto::Delete,
                    Some(precondition.server_id),
                    error,
                )
            })?;
        let revision = removed.revision;
        let mut server = before_delete.server;
        server.summary.registry_revision = revision;
        server.summary.enabled = false;
        server.summary.state = McpConnectionStateDto::Disabled;
        server.summary.updated_at_ms = now_ms();
        Ok(McpServerDetailsOutput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            registry_revision: revision,
            server,
        })
    }

    fn set_enabled_with_precondition(
        &self,
        precondition: &McpRegistryMutationPrecondition,
        enabled: bool,
        operation: McpManagementOperationDto,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.registry
            .set_enabled(precondition, enabled)
            .map_err(|error| {
                self.registry_failure(operation, Some(precondition.server_id), error)
            })?;
        self.details_for(precondition.server_id, operation)
    }
}
