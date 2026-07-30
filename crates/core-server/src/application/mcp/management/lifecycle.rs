//! Explicit MCP server start and stop operations.

use super::*;

impl McpManagementService {
    pub(crate) async fn start_server(
        &self,
        input: McpServerMutationInput,
        operation: McpManagementOperationDto,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.ensure_mutations_open(operation)?;
        ensure_schema_version(input.schema_version, operation)?;
        let mutation_server_id = parse_server_id(&input.server_id, operation)?;
        let _active_mutation = self.begin_server_mutation(mutation_server_id, operation)?;
        let server_id = self.validate_mutation_input(&input, operation)?;
        self.ensure_launch_authorized(server_id, operation)?;
        match operation {
            McpManagementOperationDto::Start => self.manager.start(server_id).await,
            McpManagementOperationDto::Restart => self.manager.restart(server_id).await,
            _ => {
                return Err(self.failure(
                    operation,
                    McpManagementErrorCodeDto::InvalidInput,
                    McpManagementRecoveryDto::FixInput,
                    "The MCP management operation is invalid.",
                    Some(server_id),
                ))
            }
        }
        .map_err(|error| self.manager_failure(operation, Some(server_id), error))?;
        self.details_for(server_id, operation)
    }

    pub(crate) async fn stop_server(
        &self,
        input: McpServerMutationInput,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::Stop)?;
        ensure_schema_version(input.schema_version, McpManagementOperationDto::Stop)?;
        let mutation_server_id =
            parse_server_id(&input.server_id, McpManagementOperationDto::Stop)?;
        let _active_mutation =
            self.begin_server_mutation(mutation_server_id, McpManagementOperationDto::Stop)?;
        let server_id = self.validate_mutation_input(&input, McpManagementOperationDto::Stop)?;
        self.manager.stop(server_id).await.map_err(|error| {
            self.manager_failure(McpManagementOperationDto::Stop, Some(server_id), error)
        })?;
        self.details_for(server_id, McpManagementOperationDto::Stop)
    }
}
