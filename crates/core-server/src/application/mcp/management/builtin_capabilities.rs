//! Built-in MCP capability policy management.
//!
//! `user_allowed` is a durable permission to request task-scoped activation. It is deliberately
//! independent from the external Server Registry's `enabled` and connection lifecycle.

use super::*;

const BROWSER_AUTOMATION_DISPLAY_NAME: &str = "Browser automation";
const BROWSER_AUTOMATION_DESCRIPTION: &str =
    "Allow the Agent to request task-scoped browser automation access.";

impl McpManagementService {
    pub(crate) fn list_builtin_capabilities(
        &self,
        input: McpBuiltinCapabilityListInput,
    ) -> Result<McpBuiltinCapabilityListOutput, McpManagementFailure> {
        ensure_schema_version(
            input.schema_version,
            McpManagementOperationDto::ListBuiltinCapabilities,
        )?;
        let (revision, records) = self
            .builtin_capability_policies
            .snapshot()
            .map_err(|error| {
                self.builtin_policy_failure(
                    McpManagementOperationDto::ListBuiltinCapabilities,
                    error,
                )
            })?;
        let capabilities = records
            .into_iter()
            .map(project_builtin_capability)
            .collect();
        Ok(McpBuiltinCapabilityListOutput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            revision,
            capabilities,
        })
    }

    pub(crate) async fn set_builtin_capability_allowed(
        &self,
        input: McpBuiltinCapabilitySetAllowedInput,
    ) -> Result<McpBuiltinCapabilityMutationOutput, McpManagementFailure> {
        let operation = McpManagementOperationDto::SetBuiltinCapabilityAllowed;
        self.ensure_mutations_open(operation)?;
        ensure_schema_version(input.schema_version, operation)?;
        let capability_id = match input.capability_id {
            McpBuiltinCapabilityIdDto::BrowserAutomation => BuiltinCapabilityId::BrowserAutomation,
        };
        let (revision, record) = self
            .builtin_capability_policies
            .set_allowed(capability_id, input.expected_policy_revision, input.allowed)
            .map_err(|error| self.builtin_policy_failure(operation, error))?;
        if !record.user_allowed {
            if let Some(coordinator) = &self.browser_risk_coordinator {
                coordinator.cancel_all();
            }
            let runtime_id = CoreBuiltinCapabilityId::parse(record.capability_id.as_str())
                .map_err(|_| {
                    self.failure(
                        operation,
                        McpManagementErrorCodeDto::InternalSafeError,
                        McpManagementRecoveryDto::DoNotRetry,
                        "The built-in MCP capability runtime identity is invalid.",
                        None,
                    )
                })?;
            self.builtin_capability_runtime
                .revoke_grants(&runtime_id)
                .map_err(|_| {
                    self.failure(
                        operation,
                        McpManagementErrorCodeDto::InternalSafeError,
                        McpManagementRecoveryDto::Retry,
                        "The built-in MCP capability grants could not be revoked.",
                        None,
                    )
                })?;
            if let Some(runtime) = &self.managed_playwright_runtime {
                runtime.stop().await.map_err(|_| {
                    self.failure(
                        operation,
                        McpManagementErrorCodeDto::InternalSafeError,
                        McpManagementRecoveryDto::Retry,
                        "The managed browser automation could not be stopped safely.",
                        None,
                    )
                })?;
            }
        }
        Ok(McpBuiltinCapabilityMutationOutput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            revision,
            capability: project_builtin_capability(record),
        })
    }
}

fn project_builtin_capability(
    record: BuiltinCapabilityPolicyRecord,
) -> McpBuiltinCapabilityListItem {
    let capability_id = match record.capability_id {
        BuiltinCapabilityId::BrowserAutomation => McpBuiltinCapabilityIdDto::BrowserAutomation,
    };
    McpBuiltinCapabilityListItem {
        schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
        kind: McpManagementEntryKindDto::BuiltinCapability,
        capability_id,
        display_name: BROWSER_AUTOMATION_DISPLAY_NAME.to_string(),
        description: BROWSER_AUTOMATION_DESCRIPTION.to_string(),
        user_allowed: record.user_allowed,
        policy_version: record.policy_version,
        policy_revision: record.policy_revision,
    }
}
