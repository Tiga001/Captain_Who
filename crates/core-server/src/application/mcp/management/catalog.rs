//! Catalog paging, cursor validation, and tool summary projection.

use super::*;

impl McpManagementService {
    pub(crate) fn list_tools(
        &self,
        input: McpCatalogToolsPageInput,
    ) -> Result<McpCatalogToolsPageOutput, McpManagementFailure> {
        ensure_schema_version(input.schema_version, McpManagementOperationDto::ListTools)?;
        let server_id = parse_server_id(&input.server_id, McpManagementOperationDto::ListTools)?;
        self.project_catalog_page(
            server_id,
            input.cursor.as_deref(),
            usize::try_from(input.limit).unwrap_or(usize::MAX),
            McpManagementOperationDto::ListTools,
        )
    }

    pub(crate) async fn refresh_catalog(
        &self,
        input: McpServerMutationInput,
    ) -> Result<McpCatalogToolsPageOutput, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::RefreshCatalog)?;
        ensure_schema_version(
            input.schema_version,
            McpManagementOperationDto::RefreshCatalog,
        )?;
        let mutation_server_id =
            parse_server_id(&input.server_id, McpManagementOperationDto::RefreshCatalog)?;
        let _active_mutation = self.begin_server_mutation(
            mutation_server_id,
            McpManagementOperationDto::RefreshCatalog,
        )?;
        let server_id =
            self.validate_mutation_input(&input, McpManagementOperationDto::RefreshCatalog)?;
        self.manager.refresh(server_id).await.map_err(|error| {
            self.manager_failure(
                McpManagementOperationDto::RefreshCatalog,
                Some(server_id),
                error,
            )
        })?;
        self.project_catalog_page(
            server_id,
            None,
            MAX_TOOL_PAGE_SIZE,
            McpManagementOperationDto::RefreshCatalog,
        )
    }

    fn project_catalog_page(
        &self,
        server_id: McpServerId,
        cursor: Option<&str>,
        limit: usize,
        operation: McpManagementOperationDto,
    ) -> Result<McpCatalogToolsPageOutput, McpManagementFailure> {
        if limit == 0 || limit > MAX_TOOL_PAGE_SIZE {
            return Err(self.failure(
                operation,
                McpManagementErrorCodeDto::InvalidInput,
                McpManagementRecoveryDto::FixInput,
                "The MCP tool page size is invalid.",
                Some(server_id),
            ));
        }
        let before = self.persisted(server_id, operation)?;
        let catalog = self
            .manager
            .catalog(server_id)
            .map_err(|error| self.manager_failure(operation, Some(server_id), error))?;
        let after = self.persisted(server_id, operation)?;
        if !same_registry_identity(&before.entry, &after.entry) {
            return Err(self.stale_catalog_failure(server_id, operation));
        }
        let catalog = match catalog {
            Some(catalog) if catalog_matches_registry(&catalog, &after.entry) => catalog,
            Some(_) => return Err(self.stale_catalog_failure(server_id, operation)),
            None => {
                let mut empty = McpCatalogSnapshot::empty(server_id);
                empty.source_config_epoch = Some(after.entry.config_epoch);
                empty.source_registry_revision = Some(after.entry.revision);
                empty.source_config_digest = Some(after.entry.config_digest.clone());
                empty
            }
        };
        let offset = match cursor {
            Some(cursor) => decode_catalog_cursor(cursor, &catalog, operation)?,
            None => 0,
        };
        if offset > catalog.tools.len() {
            return Err(self.failure(
                operation,
                McpManagementErrorCodeDto::Conflict,
                McpManagementRecoveryDto::Refresh,
                "The MCP tool page cursor is stale.",
                Some(server_id),
            ));
        }
        let end = offset.saturating_add(limit).min(catalog.tools.len());
        let completeness = catalog_completeness(&catalog.completeness);
        let mut tools = Vec::with_capacity(end.saturating_sub(offset));
        for tool in &catalog.tools[offset..end] {
            let (description, description_truncated) = truncate_text(
                tool.descriptor.description.as_deref().unwrap_or_default(),
                MAX_SAFE_DESCRIPTION_BYTES,
            );
            let diagnostic_codes = catalog
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic
                        .raw_names
                        .iter()
                        .any(|name| name == &tool.raw_name)
                        || diagnostic
                            .model_name
                            .as_ref()
                            .is_some_and(|name| name == &tool.model_name)
                })
                .map(|diagnostic| diagnostic_code(diagnostic.kind).to_string())
                .collect();
            tools.push(McpToolSummaryView {
                server_id: server_id.to_string(),
                raw_name: bounded_text(&tool.raw_name, 1024),
                model_name: bounded_text(&tool.model_name, 64),
                routable: tool.routable,
                disabled: !tool.routable,
                schema_digest_prefix: tool.schema_digest.as_str()[..12].to_string(),
                description,
                description_truncated,
                diagnostic_codes,
                catalog_generation: catalog.generation,
                catalog_completeness: completeness,
            });
        }
        let next_cursor = (end < catalog.tools.len())
            .then(|| encode_catalog_cursor(&catalog, end))
            .transpose()
            .map_err(|_| {
                self.failure(
                    operation,
                    McpManagementErrorCodeDto::InternalSafeError,
                    McpManagementRecoveryDto::Retry,
                    "The MCP tool page cursor could not be created.",
                    Some(server_id),
                )
            })?;
        Ok(McpCatalogToolsPageOutput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            server_id: server_id.to_string(),
            catalog_generation: catalog.generation,
            catalog_completeness: completeness,
            tools,
            next_cursor,
        })
    }

    fn stale_catalog_failure(
        &self,
        server_id: McpServerId,
        operation: McpManagementOperationDto,
    ) -> McpManagementFailure {
        self.failure(
            operation,
            McpManagementErrorCodeDto::Conflict,
            McpManagementRecoveryDto::Refresh,
            "The MCP tool catalog is stale. Refresh before retrying.",
            Some(server_id),
        )
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogCursor {
    schema_version: u32,
    server_id: String,
    generation: u64,
    catalog_digest: Option<String>,
    offset: usize,
}

pub(super) fn encode_catalog_cursor(
    catalog: &McpCatalogSnapshot,
    offset: usize,
) -> Result<String, serde_json::Error> {
    let encoded = serde_json::to_vec(&CatalogCursor {
        schema_version: CURSOR_SCHEMA_VERSION,
        server_id: catalog.server_id.to_string(),
        generation: catalog.generation,
        catalog_digest: catalog.content_digest.as_ref().map(ToString::to_string),
        offset,
    })?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(encoded))
}

pub(super) fn decode_catalog_cursor(
    cursor: &str,
    catalog: &McpCatalogSnapshot,
    operation: McpManagementOperationDto,
) -> Result<usize, McpManagementFailure> {
    if cursor.len() > MAX_HOST_CURSOR_BYTES {
        return Err(standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The MCP tool page cursor is invalid.",
            Some(catalog.server_id),
        ));
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| {
            standalone_failure(
                operation,
                McpManagementErrorCodeDto::InvalidInput,
                McpManagementRecoveryDto::FixInput,
                "The MCP tool page cursor is invalid.",
                Some(catalog.server_id),
            )
        })?;
    let decoded: CatalogCursor = serde_json::from_slice(&bytes).map_err(|_| {
        standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The MCP tool page cursor is invalid.",
            Some(catalog.server_id),
        )
    })?;
    if decoded.schema_version != CURSOR_SCHEMA_VERSION
        || decoded.server_id != catalog.server_id.to_string()
        || decoded.generation != catalog.generation
        || decoded.catalog_digest != catalog.content_digest.as_ref().map(ToString::to_string)
    {
        return Err(standalone_failure(
            operation,
            McpManagementErrorCodeDto::Conflict,
            McpManagementRecoveryDto::Refresh,
            "The MCP tool page cursor is stale.",
            Some(catalog.server_id),
        ));
    }
    Ok(decoded.offset)
}
