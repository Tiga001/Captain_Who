//! Management input, identity, precondition, and cursor validation.

use super::*;

pub(super) fn ensure_schema_version(
    schema_version: u32,
    operation: McpManagementOperationDto,
) -> Result<(), McpManagementFailure> {
    if schema_version == MCP_MANAGEMENT_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The MCP management schema version is unsupported.",
            None,
        ))
    }
}

pub(super) fn parse_server_id(
    value: &str,
    operation: McpManagementOperationDto,
) -> Result<McpServerId, McpManagementFailure> {
    McpServerId::from_str(value).map_err(|_| {
        standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The MCP server identity is invalid.",
            None,
        )
    })
}

pub(super) fn parse_uuid_v4(
    value: &str,
    operation: McpManagementOperationDto,
) -> Result<Uuid, McpManagementFailure> {
    let id = Uuid::parse_str(value).map_err(|_| {
        standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The launch authorization identity is invalid.",
            None,
        )
    })?;
    if id.get_version() == Some(uuid::Version::Random) && id.to_string() == value {
        Ok(id)
    } else {
        Err(standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The launch authorization identity is invalid.",
            None,
        ))
    }
}

pub(super) fn parse_precondition(
    server_id: &str,
    input: &McpServerMutationPrecondition,
    operation: McpManagementOperationDto,
) -> Result<McpRegistryMutationPrecondition, McpManagementFailure> {
    let server_id = parse_server_id(server_id, operation)?;
    if input.expected_registry_revision == 0 {
        return Err(standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The MCP Registry revision is invalid.",
            Some(server_id),
        ));
    }
    let expected_config_epoch =
        McpConfigEpoch::from_str(&input.expected_config_epoch).map_err(|_| {
            standalone_failure(
                operation,
                McpManagementErrorCodeDto::InvalidInput,
                McpManagementRecoveryDto::FixInput,
                "The MCP configuration epoch is invalid.",
                Some(server_id),
            )
        })?;
    let expected_config_digest =
        McpConfigDigest::from_str(&input.expected_config_digest).map_err(|_| {
            standalone_failure(
                operation,
                McpManagementErrorCodeDto::InvalidInput,
                McpManagementRecoveryDto::FixInput,
                "The MCP configuration digest is invalid.",
                Some(server_id),
            )
        })?;
    Ok(McpRegistryMutationPrecondition {
        server_id,
        expected_revision: input.expected_registry_revision,
        expected_config_epoch,
        expected_config_digest,
    })
}

pub(super) fn project_precondition(entry: &McpRegistryEntry) -> McpServerMutationPrecondition {
    McpServerMutationPrecondition {
        expected_registry_revision: entry.revision,
        expected_config_epoch: entry.config_epoch.to_string(),
        expected_config_digest: entry.config_digest.to_string(),
    }
}

pub(super) fn ensure_registry_precondition(
    entry: &McpRegistryEntry,
    precondition: &McpRegistryMutationPrecondition,
) -> Result<(), ()> {
    if entry.config.id == precondition.server_id
        && entry.revision == precondition.expected_revision
        && entry.config_epoch == precondition.expected_config_epoch
        && entry.config_digest == precondition.expected_config_digest
    {
        Ok(())
    } else {
        Err(())
    }
}

pub(super) fn same_registry_identity(left: &McpRegistryEntry, right: &McpRegistryEntry) -> bool {
    left.config.id == right.config.id
        && left.revision == right.revision
        && left.config_epoch == right.config_epoch
        && left.config_digest == right.config_digest
}

pub(super) fn catalog_matches_registry(
    catalog: &McpCatalogSnapshot,
    entry: &McpRegistryEntry,
) -> bool {
    catalog.server_id == entry.config.id
        && catalog.source_config_epoch == Some(entry.config_epoch)
        && catalog.source_registry_revision == Some(entry.revision)
        && catalog.source_config_digest.as_ref() == Some(&entry.config_digest)
}

pub(super) fn validate_editable_fields(
    display_name: &str,
    executable: &str,
    arguments: &[String],
    cwd: &str,
    operation: McpManagementOperationDto,
) -> Result<(), McpManagementFailure> {
    let arguments_bytes = arguments
        .iter()
        .try_fold(0usize, |total, argument| total.checked_add(argument.len()));
    let valid = !display_name.trim().is_empty()
        && display_name.len() <= MAX_DISPLAY_NAME_BYTES
        && !display_name.contains('\0')
        && !display_name.chars().any(is_unsafe_renderer_text_character)
        && !executable.is_empty()
        && executable.len() <= MAX_PATH_BYTES
        && !executable.contains('\0')
        && !cwd.is_empty()
        && cwd.len() <= MAX_PATH_BYTES
        && !cwd.contains('\0')
        && arguments.len() <= MAX_ARGUMENTS
        && arguments
            .iter()
            .all(|argument| argument.len() <= MAX_ARGUMENT_BYTES && !argument.contains('\0'))
        && arguments_bytes.is_some_and(|total| total <= MAX_ARGUMENTS_TOTAL_BYTES);
    if valid {
        Ok(())
    } else {
        Err(standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The MCP server configuration exceeds the allowed limits.",
            None,
        ))
    }
}

pub(super) fn approval_mode(value: McpApprovalModeDto) -> McpApprovalMode {
    match value {
        McpApprovalModeDto::Prompt => McpApprovalMode::Prompt,
        McpApprovalModeDto::Deny => McpApprovalMode::Deny,
    }
}
