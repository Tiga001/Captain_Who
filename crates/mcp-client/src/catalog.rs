use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::digest::{canonical_json_bytes, catalog_digest, schema_digest, tool_name_hash};
use crate::limits::validate_schema_value;
use crate::{
    McpCatalogDigest, McpConfigDigest, McpConfigEpoch, McpError, McpPeer, McpSchemaDigest,
    McpSecurityLimits, McpServerId, McpToolDescriptor,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolId {
    pub server_id: McpServerId,
    pub raw_name: String,
}

/// A catalog-bound request to invoke one MCP tool.
///
/// The optimistic snapshot fields make stale model-visible tool definitions
/// fail closed. Routing still uses `tool_id` and never parses `model_name`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCatalogToolCallIdentity {
    pub tool_id: McpToolId,
    pub expected_config_epoch: McpConfigEpoch,
    pub expected_registry_revision: u64,
    pub expected_config_digest: McpConfigDigest,
    pub expected_catalog_generation: u64,
    pub expected_catalog_digest: McpCatalogDigest,
    pub expected_schema_digest: McpSchemaDigest,
    pub expected_model_name: String,
}

impl fmt::Debug for McpCatalogToolCallIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpCatalogToolCallIdentity")
            .field("server_id", &self.tool_id.server_id)
            .field("raw_name", &"<redacted>")
            .field("expected_config_epoch", &self.expected_config_epoch)
            .field(
                "expected_registry_revision",
                &self.expected_registry_revision,
            )
            .field("expected_config_digest", &self.expected_config_digest)
            .field(
                "expected_catalog_generation",
                &self.expected_catalog_generation,
            )
            .field("expected_catalog_digest", &self.expected_catalog_digest)
            .field("expected_schema_digest", &self.expected_schema_digest)
            .field("expected_model_name", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCatalogToolCall {
    pub tool_id: McpToolId,
    pub expected_config_epoch: McpConfigEpoch,
    pub expected_registry_revision: u64,
    pub expected_config_digest: McpConfigDigest,
    pub expected_catalog_generation: u64,
    pub expected_catalog_digest: McpCatalogDigest,
    pub expected_schema_digest: McpSchemaDigest,
    pub expected_model_name: String,
    #[serde(default)]
    pub arguments: Value,
    pub timeout_ms: Option<u64>,
}

impl fmt::Debug for McpCatalogToolCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpCatalogToolCall")
            .field("server_id", &self.tool_id.server_id)
            .field("raw_name", &"<redacted>")
            .field("expected_config_epoch", &self.expected_config_epoch)
            .field(
                "expected_registry_revision",
                &self.expected_registry_revision,
            )
            .field("expected_config_digest", &self.expected_config_digest)
            .field(
                "expected_catalog_generation",
                &self.expected_catalog_generation,
            )
            .field("expected_catalog_digest", &self.expected_catalog_digest)
            .field("expected_schema_digest", &self.expected_schema_digest)
            .field("expected_model_name", &"<redacted>")
            .field("arguments", &"<redacted>")
            .field("timeout_ms", &self.timeout_ms)
            .finish()
    }
}

impl McpCatalogToolCall {
    pub fn new(identity: McpCatalogToolCallIdentity, arguments: Value) -> Self {
        Self {
            tool_id: identity.tool_id,
            expected_config_epoch: identity.expected_config_epoch,
            expected_registry_revision: identity.expected_registry_revision,
            expected_config_digest: identity.expected_config_digest,
            expected_catalog_generation: identity.expected_catalog_generation,
            expected_catalog_digest: identity.expected_catalog_digest,
            expected_schema_digest: identity.expected_schema_digest,
            expected_model_name: identity.expected_model_name,
            arguments,
            timeout_ms: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpCatalogIssue {
    RequestFailed,
    RepeatedCursor,
    CursorTooLarge,
    PageLimitExceeded,
    ToolLimitExceeded,
    PageSchemaLimitExceeded,
    TotalSchemaLimitExceeded,
    PageDescriptorLimitExceeded,
    TotalDescriptorLimitExceeded,
    InvalidSchema,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpCatalogCompleteness {
    Complete,
    Partial(McpCatalogIssue),
    Stale(McpCatalogIssue),
    Failed(McpCatalogIssue),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpCatalogDiagnosticKind {
    InvalidRawName,
    DescriptionLimitExceeded,
    InvalidSchema,
    DuplicateRawName,
    NormalizationCollision,
    ModelNameCollision,
    ReservedModelName,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCatalogDiagnostic {
    pub kind: McpCatalogDiagnosticKind,
    pub raw_names: Vec<String>,
    pub model_name: Option<String>,
    pub occurrence_count: usize,
}

impl fmt::Debug for McpCatalogDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpCatalogDiagnostic")
            .field("kind", &self.kind)
            .field("raw_name_count", &self.raw_names.len())
            .field("has_model_name", &self.model_name.is_some())
            .field("occurrence_count", &self.occurrence_count)
            .finish()
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCatalogTool {
    pub id: McpToolId,
    pub raw_name: String,
    pub model_name: String,
    pub schema_digest: McpSchemaDigest,
    pub descriptor: McpToolDescriptor,
    pub routable: bool,
}

impl fmt::Debug for McpCatalogTool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpCatalogTool")
            .field("server_id", &self.id.server_id)
            .field("raw_name", &"<redacted>")
            .field("model_name", &self.model_name)
            .field("schema_digest", &self.schema_digest)
            .field("descriptor", &"<redacted>")
            .field("routable", &self.routable)
            .finish()
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCatalogSnapshot {
    pub server_id: McpServerId,
    /// Opaque configuration incarnation that produced this catalog.
    pub source_config_epoch: Option<McpConfigEpoch>,
    /// Registry-wide ordering revision that produced this catalog.
    pub source_registry_revision: Option<u64>,
    /// Digest of the Registry configuration that produced this catalog.
    /// Protocol-only discovery leaves it unset; Connection Manager snapshots
    /// always bind it before publication or routing.
    pub source_config_digest: Option<McpConfigDigest>,
    pub generation: u64,
    pub completeness: McpCatalogCompleteness,
    pub tools: Vec<McpCatalogTool>,
    pub diagnostics: Vec<McpCatalogDiagnostic>,
    pub page_count: usize,
    pub total_schema_bytes: usize,
    pub total_descriptor_bytes: usize,
    pub content_digest: Option<McpCatalogDigest>,
}

impl McpCatalogSnapshot {
    pub fn empty(server_id: McpServerId) -> Self {
        Self {
            server_id,
            source_config_epoch: None,
            source_registry_revision: None,
            source_config_digest: None,
            generation: 0,
            completeness: McpCatalogCompleteness::Failed(McpCatalogIssue::RequestFailed),
            tools: Vec::new(),
            diagnostics: Vec::new(),
            page_count: 0,
            total_schema_bytes: 0,
            total_descriptor_bytes: 0,
            content_digest: None,
        }
    }

    pub fn resolve_model_name(&self, model_name: &str) -> Option<&McpToolId> {
        if self.completeness != McpCatalogCompleteness::Complete {
            return None;
        }
        self.tools
            .iter()
            .find(|tool| tool.routable && tool.model_name == model_name)
            .map(|tool| &tool.id)
    }
}

impl fmt::Debug for McpCatalogSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpCatalogSnapshot")
            .field("server_id", &self.server_id)
            .field("source_config_epoch", &self.source_config_epoch)
            .field("source_registry_revision", &self.source_registry_revision)
            .field("source_config_digest", &self.source_config_digest)
            .field("generation", &self.generation)
            .field("completeness", &self.completeness)
            .field("tool_count", &self.tools.len())
            .field("diagnostic_count", &self.diagnostics.len())
            .field("page_count", &self.page_count)
            .field("total_schema_bytes", &self.total_schema_bytes)
            .field("total_descriptor_bytes", &self.total_descriptor_bytes)
            .field("content_digest", &self.content_digest)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpCatalogLimits {
    pub max_pages: usize,
    pub max_tools: usize,
    pub max_schema_bytes_per_page: usize,
    pub max_total_schema_bytes: usize,
    pub max_descriptor_bytes_per_page: usize,
    pub max_total_descriptor_bytes: usize,
    pub max_cursor_bytes: usize,
    pub max_model_name_bytes: usize,
}

impl Default for McpCatalogLimits {
    fn default() -> Self {
        let security = crate::McpSecurityLimits::default();
        Self {
            max_pages: security.max_catalog_pages,
            max_tools: security.max_tools,
            max_schema_bytes_per_page: security.max_schema_bytes_per_page,
            max_total_schema_bytes: security.max_total_schema_bytes,
            max_descriptor_bytes_per_page: security.max_descriptor_bytes_per_page,
            max_total_descriptor_bytes: security.max_total_descriptor_bytes,
            max_cursor_bytes: security.max_cursor_bytes,
            max_model_name_bytes: security.max_model_name_bytes,
        }
    }
}

impl McpCatalogLimits {
    pub fn validate(&self) -> Result<(), McpError> {
        if self.max_pages == 0
            || self.max_tools == 0
            || self.max_schema_bytes_per_page == 0
            || self.max_total_schema_bytes == 0
            || self.max_descriptor_bytes_per_page == 0
            || self.max_total_descriptor_bytes == 0
            || self.max_cursor_bytes == 0
            || !(53..=64).contains(&self.max_model_name_bytes)
        {
            return Err(McpError::config("MCP catalog limits are invalid"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct McpCatalogPolicy {
    pub limits: McpCatalogLimits,
    pub reserved_model_names: BTreeSet<String>,
}

#[derive(Clone, Copy)]
struct CatalogMeasurements {
    page_count: usize,
    total_schema_bytes: usize,
    total_descriptor_bytes: usize,
}

impl CatalogMeasurements {
    fn new(page_count: usize, total_schema_bytes: usize, total_descriptor_bytes: usize) -> Self {
        Self {
            page_count,
            total_schema_bytes,
            total_descriptor_bytes,
        }
    }
}

struct CatalogDescriptorCandidate {
    descriptor: McpToolDescriptor,
    issues: Vec<McpCatalogDiagnosticKind>,
}

#[cfg(test)]
pub(crate) async fn discover_catalog(
    peer: &dyn McpPeer,
    server_id: McpServerId,
    previous: Option<&McpCatalogSnapshot>,
    policy: &McpCatalogPolicy,
) -> Result<McpCatalogSnapshot, McpError> {
    discover_catalog_with_limits(
        peer,
        server_id,
        previous,
        policy,
        &McpSecurityLimits::default(),
    )
    .await
}

pub(crate) async fn discover_catalog_with_limits(
    peer: &dyn McpPeer,
    server_id: McpServerId,
    previous: Option<&McpCatalogSnapshot>,
    policy: &McpCatalogPolicy,
    security_limits: &McpSecurityLimits,
) -> Result<McpCatalogSnapshot, McpError> {
    policy.limits.validate()?;
    security_limits.validate()?;
    if !peer.protocol_snapshot().capabilities.tools {
        return complete_snapshot(server_id, previous, Vec::new(), 0, 0, 0, policy);
    }
    let mut cursor = None;
    let mut seen_cursors = BTreeSet::new();
    let mut descriptors = Vec::new();
    let mut page_count = 0_usize;
    let mut total_schema_bytes = 0_usize;
    let mut total_descriptor_bytes = 0_usize;

    loop {
        if page_count >= policy.limits.max_pages {
            return incomplete_snapshot(
                server_id,
                previous,
                descriptors,
                McpCatalogIssue::PageLimitExceeded,
                CatalogMeasurements::new(page_count, total_schema_bytes, total_descriptor_bytes),
                policy,
            );
        }

        let page = match peer.list_tools(cursor.clone()).await {
            Ok(page) => page,
            Err(_) => {
                return incomplete_snapshot(
                    server_id,
                    previous,
                    descriptors,
                    McpCatalogIssue::RequestFailed,
                    CatalogMeasurements::new(
                        page_count,
                        total_schema_bytes,
                        total_descriptor_bytes,
                    ),
                    policy,
                );
            }
        };
        page_count = page_count
            .checked_add(1)
            .ok_or_else(|| McpError::protocol("MCP catalog page count overflowed"))?;

        let page_candidates = page
            .tools
            .into_iter()
            .map(|descriptor| prepare_descriptor(descriptor, security_limits))
            .collect::<Vec<_>>();
        let mut page_schema_bytes = 0_usize;
        let mut page_descriptor_bytes = 0_usize;
        for candidate in &page_candidates {
            let bytes = schema_bytes(&candidate.descriptor).map_err(|_| {
                McpError::protocol("MCP tool schema could not be normalized safely")
            })?;
            page_schema_bytes = page_schema_bytes
                .checked_add(bytes)
                .ok_or_else(|| McpError::protocol("MCP schema byte count overflowed"))?;
            page_descriptor_bytes = page_descriptor_bytes
                .checked_add(descriptor_sort_key(&candidate.descriptor)?.len())
                .ok_or_else(|| McpError::protocol("MCP descriptor byte count overflowed"))?;
        }
        if page_schema_bytes > policy.limits.max_schema_bytes_per_page {
            return incomplete_snapshot(
                server_id,
                previous,
                descriptors,
                McpCatalogIssue::PageSchemaLimitExceeded,
                CatalogMeasurements::new(page_count, total_schema_bytes, total_descriptor_bytes),
                policy,
            );
        }
        if page_descriptor_bytes > policy.limits.max_descriptor_bytes_per_page {
            return incomplete_snapshot(
                server_id,
                previous,
                descriptors,
                McpCatalogIssue::PageDescriptorLimitExceeded,
                CatalogMeasurements::new(page_count, total_schema_bytes, total_descriptor_bytes),
                policy,
            );
        }

        let proposed_tools = descriptors
            .len()
            .checked_add(page_candidates.len())
            .ok_or_else(|| McpError::protocol("MCP tool count overflowed"))?;
        if proposed_tools > policy.limits.max_tools {
            return incomplete_snapshot(
                server_id,
                previous,
                descriptors,
                McpCatalogIssue::ToolLimitExceeded,
                CatalogMeasurements::new(page_count, total_schema_bytes, total_descriptor_bytes),
                policy,
            );
        }
        let proposed_schema_bytes = total_schema_bytes
            .checked_add(page_schema_bytes)
            .ok_or_else(|| McpError::protocol("MCP schema byte count overflowed"))?;
        if proposed_schema_bytes > policy.limits.max_total_schema_bytes {
            return incomplete_snapshot(
                server_id,
                previous,
                descriptors,
                McpCatalogIssue::TotalSchemaLimitExceeded,
                CatalogMeasurements::new(page_count, total_schema_bytes, total_descriptor_bytes),
                policy,
            );
        }
        let proposed_descriptor_bytes =
            total_descriptor_bytes
                .checked_add(page_descriptor_bytes)
                .ok_or_else(|| McpError::protocol("MCP descriptor byte count overflowed"))?;
        if proposed_descriptor_bytes > policy.limits.max_total_descriptor_bytes {
            return incomplete_snapshot(
                server_id,
                previous,
                descriptors,
                McpCatalogIssue::TotalDescriptorLimitExceeded,
                CatalogMeasurements::new(page_count, total_schema_bytes, total_descriptor_bytes),
                policy,
            );
        }
        descriptors.extend(page_candidates);
        total_schema_bytes = proposed_schema_bytes;
        total_descriptor_bytes = proposed_descriptor_bytes;

        let Some(next_cursor) = page.next_cursor else {
            break;
        };
        if next_cursor.len() > policy.limits.max_cursor_bytes {
            return incomplete_snapshot(
                server_id,
                previous,
                descriptors,
                McpCatalogIssue::CursorTooLarge,
                CatalogMeasurements::new(page_count, total_schema_bytes, total_descriptor_bytes),
                policy,
            );
        }
        if !seen_cursors.insert(next_cursor.clone()) {
            return incomplete_snapshot(
                server_id,
                previous,
                descriptors,
                McpCatalogIssue::RepeatedCursor,
                CatalogMeasurements::new(page_count, total_schema_bytes, total_descriptor_bytes),
                policy,
            );
        }
        cursor = Some(next_cursor);
    }

    complete_snapshot(
        server_id,
        previous,
        descriptors,
        page_count,
        total_schema_bytes,
        total_descriptor_bytes,
        policy,
    )
}

fn complete_snapshot(
    server_id: McpServerId,
    previous: Option<&McpCatalogSnapshot>,
    descriptors: Vec<CatalogDescriptorCandidate>,
    page_count: usize,
    total_schema_bytes: usize,
    total_descriptor_bytes: usize,
    policy: &McpCatalogPolicy,
) -> Result<McpCatalogSnapshot, McpError> {
    let (tools, diagnostics) = build_tools(server_id, descriptors, policy)?;
    let digest = effective_catalog_digest(&tools)?;
    let generation = match previous {
        Some(previous) if previous.content_digest.as_ref() == Some(&digest) => previous.generation,
        Some(previous) => previous
            .generation
            .checked_add(1)
            .ok_or_else(|| McpError::protocol("MCP catalog generation exhausted"))?,
        None => 1,
    };
    Ok(McpCatalogSnapshot {
        server_id,
        source_config_epoch: None,
        source_registry_revision: None,
        source_config_digest: None,
        generation,
        completeness: McpCatalogCompleteness::Complete,
        tools,
        diagnostics,
        page_count,
        total_schema_bytes,
        total_descriptor_bytes,
        content_digest: Some(digest),
    })
}

fn incomplete_snapshot(
    server_id: McpServerId,
    previous: Option<&McpCatalogSnapshot>,
    descriptors: Vec<CatalogDescriptorCandidate>,
    issue: McpCatalogIssue,
    measurements: CatalogMeasurements,
    policy: &McpCatalogPolicy,
) -> Result<McpCatalogSnapshot, McpError> {
    let previous_generation = previous.map_or(0, |snapshot| snapshot.generation);
    if let Some(previous) = previous.filter(|snapshot| snapshot.content_digest.is_some()) {
        let mut stale = previous.clone();
        stale.completeness = McpCatalogCompleteness::Stale(issue);
        stale.page_count = measurements.page_count;
        stale.total_schema_bytes = measurements.total_schema_bytes;
        stale.total_descriptor_bytes = measurements.total_descriptor_bytes;
        return Ok(stale);
    }
    if descriptors.is_empty() {
        return Ok(McpCatalogSnapshot {
            server_id,
            source_config_epoch: None,
            source_registry_revision: None,
            source_config_digest: None,
            generation: previous_generation,
            completeness: McpCatalogCompleteness::Failed(issue),
            tools: Vec::new(),
            diagnostics: Vec::new(),
            page_count: measurements.page_count,
            total_schema_bytes: measurements.total_schema_bytes,
            total_descriptor_bytes: measurements.total_descriptor_bytes,
            content_digest: None,
        });
    }
    let (tools, diagnostics) = build_tools(server_id, descriptors, policy)?;
    Ok(McpCatalogSnapshot {
        server_id,
        source_config_epoch: None,
        source_registry_revision: None,
        source_config_digest: None,
        generation: previous_generation,
        completeness: McpCatalogCompleteness::Partial(issue),
        tools,
        diagnostics,
        page_count: measurements.page_count,
        total_schema_bytes: measurements.total_schema_bytes,
        total_descriptor_bytes: measurements.total_descriptor_bytes,
        content_digest: None,
    })
}

fn build_tools(
    server_id: McpServerId,
    mut descriptors: Vec<CatalogDescriptorCandidate>,
    policy: &McpCatalogPolicy,
) -> Result<(Vec<McpCatalogTool>, Vec<McpCatalogDiagnostic>), McpError> {
    descriptors.sort_by(|left, right| {
        left.descriptor
            .name
            .cmp(&right.descriptor.name)
            .then_with(|| {
                descriptor_sort_key(&left.descriptor)
                    .unwrap_or_default()
                    .cmp(&descriptor_sort_key(&right.descriptor).unwrap_or_default())
            })
    });

    let mut diagnostics = Vec::new();
    let mut grouped = BTreeMap::<String, Vec<CatalogDescriptorCandidate>>::new();
    for candidate in descriptors {
        for kind in &candidate.issues {
            diagnostics.push(McpCatalogDiagnostic {
                kind: *kind,
                raw_names: vec![safe_diagnostic_name(&candidate.descriptor.name)],
                model_name: None,
                occurrence_count: 1,
            });
        }
        grouped
            .entry(candidate.descriptor.name.clone())
            .or_default()
            .push(candidate);
    }

    let mut tools = Vec::with_capacity(grouped.len());
    for (raw_name, duplicates) in grouped {
        let duplicate_count = duplicates.len();
        if duplicate_count > 1 {
            diagnostics.push(McpCatalogDiagnostic {
                kind: McpCatalogDiagnosticKind::DuplicateRawName,
                raw_names: vec![raw_name.clone()],
                model_name: None,
                occurrence_count: duplicate_count,
            });
        }
        let candidate = duplicates
            .into_iter()
            .next()
            .expect("grouped descriptor must not be empty");
        let descriptor_valid = candidate.issues.is_empty();
        let descriptor = candidate.descriptor;
        let model_name = build_model_name(server_id, &raw_name, policy.limits.max_model_name_bytes);
        let reserved = policy.reserved_model_names.contains(&model_name);
        if reserved {
            diagnostics.push(McpCatalogDiagnostic {
                kind: McpCatalogDiagnosticKind::ReservedModelName,
                raw_names: vec![raw_name.clone()],
                model_name: Some(model_name.clone()),
                occurrence_count: 1,
            });
        }
        tools.push(McpCatalogTool {
            id: McpToolId {
                server_id,
                raw_name: raw_name.clone(),
            },
            raw_name,
            model_name,
            schema_digest: schema_digest(
                &descriptor.input_schema,
                descriptor.output_schema.as_ref(),
            )?,
            descriptor,
            routable: descriptor_valid && duplicate_count == 1 && !reserved,
        });
    }

    let mut stems = BTreeMap::<String, Vec<String>>::new();
    for tool in &tools {
        stems
            .entry(normalize_stem(&tool.raw_name))
            .or_default()
            .push(tool.raw_name.clone());
    }
    for raw_names in stems.into_values().filter(|names| names.len() > 1) {
        diagnostics.push(McpCatalogDiagnostic {
            kind: McpCatalogDiagnosticKind::NormalizationCollision,
            occurrence_count: raw_names.len(),
            raw_names,
            model_name: None,
        });
    }

    let mut by_model_name = BTreeMap::<String, Vec<usize>>::new();
    for (index, tool) in tools.iter().enumerate() {
        by_model_name
            .entry(tool.model_name.clone())
            .or_default()
            .push(index);
    }
    for (model_name, indexes) in by_model_name
        .into_iter()
        .filter(|(_, indexes)| indexes.len() > 1)
    {
        let raw_names = indexes
            .iter()
            .map(|index| tools[*index].raw_name.clone())
            .collect::<Vec<_>>();
        for index in &indexes {
            tools[*index].routable = false;
        }
        diagnostics.push(McpCatalogDiagnostic {
            kind: McpCatalogDiagnosticKind::ModelNameCollision,
            occurrence_count: indexes.len(),
            raw_names,
            model_name: Some(model_name),
        });
    }

    diagnostics.sort_by(|left, right| {
        diagnostic_rank(left.kind)
            .cmp(&diagnostic_rank(right.kind))
            .then_with(|| left.raw_names.cmp(&right.raw_names))
    });
    Ok((tools, diagnostics))
}

fn prepare_descriptor(
    mut descriptor: McpToolDescriptor,
    limits: &McpSecurityLimits,
) -> CatalogDescriptorCandidate {
    let mut issues = Vec::new();
    if descriptor.name.is_empty()
        || descriptor.name.len() > limits.max_raw_tool_name_bytes
        || descriptor.name.chars().any(char::is_control)
    {
        descriptor.name = format!("invalid_{}", tool_name_hash(&descriptor.name));
        issues.push(McpCatalogDiagnosticKind::InvalidRawName);
    }

    let title_invalid = descriptor
        .title
        .as_ref()
        .is_some_and(|title| title.len() > limits.max_tool_description_bytes);
    let description_invalid = descriptor
        .description
        .as_ref()
        .is_some_and(|description| description.len() > limits.max_tool_description_bytes);
    let annotation_title_invalid = descriptor
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.title.as_ref())
        .is_some_and(|title| title.len() > limits.max_tool_description_bytes);
    if title_invalid || description_invalid || annotation_title_invalid {
        if title_invalid {
            descriptor.title = None;
        }
        if description_invalid {
            descriptor.description = None;
        }
        if annotation_title_invalid {
            if let Some(annotations) = &mut descriptor.annotations {
                annotations.title = None;
            }
        }
        issues.push(McpCatalogDiagnosticKind::DescriptionLimitExceeded);
    }

    let input_valid = validate_schema_value(&descriptor.input_schema, limits).is_ok();
    let output_valid = descriptor
        .output_schema
        .as_ref()
        .is_none_or(|schema| validate_schema_value(schema, limits).is_ok());
    if !input_valid || !output_valid {
        if !input_valid {
            descriptor.input_schema = serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            });
        }
        if !output_valid {
            descriptor.output_schema = None;
        }
        issues.push(McpCatalogDiagnosticKind::InvalidSchema);
    }

    CatalogDescriptorCandidate { descriptor, issues }
}

fn safe_diagnostic_name(value: &str) -> String {
    const MAX_BYTES: usize = 256;
    let mut output = String::new();
    for character in value.chars() {
        let character = if character.is_control() {
            '\u{fffd}'
        } else {
            character
        };
        if output.len() + character.len_utf8() > MAX_BYTES {
            break;
        }
        output.push(character);
    }
    output
}

fn effective_catalog_digest(tools: &[McpCatalogTool]) -> Result<McpCatalogDigest, McpError> {
    let value = serde_json::to_value(tools)
        .map_err(|_| McpError::protocol("MCP catalog could not be normalized"))?;
    catalog_digest(&value)
}

fn descriptor_sort_key(descriptor: &McpToolDescriptor) -> Result<Vec<u8>, McpError> {
    let value = serde_json::to_value(descriptor)
        .map_err(|_| McpError::protocol("MCP tool descriptor could not be normalized"))?;
    canonical_json_bytes(&value)
}

fn schema_bytes(descriptor: &McpToolDescriptor) -> Result<usize, McpError> {
    let input = canonical_json_bytes(&descriptor.input_schema)?.len();
    let output = descriptor
        .output_schema
        .as_ref()
        .map(canonical_json_bytes)
        .transpose()?
        .map_or(0, |bytes| bytes.len());
    input
        .checked_add(output)
        .ok_or_else(|| McpError::protocol("MCP schema byte count overflowed"))
}

fn build_model_name(server_id: McpServerId, raw_name: &str, max_bytes: usize) -> String {
    let prefix = format!("mcp__{}__", server_id.as_uuid().simple());
    let suffix = format!("_{}", tool_name_hash(raw_name));
    let stem_budget = max_bytes
        .saturating_sub(prefix.len())
        .saturating_sub(suffix.len())
        .max(1);
    let mut stem = normalize_stem(raw_name);
    stem.truncate(stem_budget);
    format!("{prefix}{stem}{suffix}")
}

fn normalize_stem(raw_name: &str) -> String {
    let mut output = String::new();
    let mut previous_separator = false;
    for byte in raw_name.bytes() {
        let normalized = if byte.is_ascii_alphanumeric() {
            previous_separator = false;
            Some(byte.to_ascii_lowercase() as char)
        } else if !previous_separator {
            previous_separator = true;
            Some('_')
        } else {
            None
        };
        if let Some(value) = normalized {
            output.push(value);
        }
    }
    let trimmed = output.trim_matches('_');
    if trimmed.is_empty() {
        "tool".to_string()
    } else {
        trimmed.to_string()
    }
}

fn diagnostic_rank(kind: McpCatalogDiagnosticKind) -> u8 {
    match kind {
        McpCatalogDiagnosticKind::InvalidRawName => 0,
        McpCatalogDiagnosticKind::DescriptionLimitExceeded => 1,
        McpCatalogDiagnosticKind::InvalidSchema => 2,
        McpCatalogDiagnosticKind::DuplicateRawName => 3,
        McpCatalogDiagnosticKind::NormalizationCollision => 4,
        McpCatalogDiagnosticKind::ModelNameCollision => 5,
        McpCatalogDiagnosticKind::ReservedModelName => 6,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use serde_json::{json, Map, Value};

    use super::*;
    use crate::{
        BoxMcpFuture, McpCancellationToken, McpCapabilitySnapshot, McpConnectionState,
        McpLifecycleKind, McpProtocolSnapshot, McpToolCall, McpToolPage, McpToolResult,
    };

    type ScriptedPage<'a> = (Option<&'a str>, Result<McpToolPage, McpError>);
    type OwnedScriptedPage = (Option<String>, Result<McpToolPage, McpError>);

    struct ScriptedPeer {
        server_id: McpServerId,
        protocol: McpProtocolSnapshot,
        pages: Mutex<VecDeque<OwnedScriptedPage>>,
        requested_cursors: Mutex<Vec<Option<String>>>,
    }

    impl ScriptedPeer {
        fn new(server_id: McpServerId, pages: Vec<ScriptedPage<'_>>) -> Arc<Self> {
            Arc::new(Self {
                server_id,
                protocol: McpProtocolSnapshot {
                    negotiated_version: "2026-07-28".to_string(),
                    lifecycle: McpLifecycleKind::Discover,
                    server: None,
                    capabilities: McpCapabilitySnapshot {
                        tools: true,
                        ..McpCapabilitySnapshot::default()
                    },
                },
                pages: Mutex::new(
                    pages
                        .into_iter()
                        .map(|(cursor, page)| (cursor.map(str::to_string), page))
                        .collect(),
                ),
                requested_cursors: Mutex::new(Vec::new()),
            })
        }
    }

    impl McpPeer for ScriptedPeer {
        fn server_id(&self) -> McpServerId {
            self.server_id
        }

        fn connection_state(&self) -> McpConnectionState {
            McpConnectionState::Ready
        }

        fn protocol_snapshot(&self) -> &McpProtocolSnapshot {
            &self.protocol
        }

        fn list_tools<'a>(&'a self, cursor: Option<String>) -> BoxMcpFuture<'a, McpToolPage> {
            Box::pin(async move {
                self.requested_cursors.lock().unwrap().push(cursor.clone());
                let (expected, result) = self
                    .pages
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unexpected tools/list call");
                assert_eq!(cursor, expected);
                result
            })
        }

        fn call_tool<'a>(
            &'a self,
            _call: McpToolCall,
            _cancellation: McpCancellationToken,
        ) -> BoxMcpFuture<'a, McpToolResult> {
            Box::pin(async { Err(McpError::protocol("not used by catalog tests")) })
        }

        fn close(&self) -> BoxMcpFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    fn page(tools: Vec<McpToolDescriptor>, next_cursor: Option<&str>) -> McpToolPage {
        McpToolPage {
            tools,
            next_cursor: next_cursor.map(str::to_string),
            ttl_ms: None,
            cache_scope: None,
        }
    }

    fn descriptor(name: &str) -> McpToolDescriptor {
        McpToolDescriptor {
            name: name.to_string(),
            title: None,
            description: None,
            input_schema: json!({"type":"object"}),
            output_schema: None,
            annotations: None,
        }
    }

    fn candidates(descriptors: Vec<McpToolDescriptor>) -> Vec<CatalogDescriptorCandidate> {
        let limits = McpSecurityLimits::default();
        descriptors
            .into_iter()
            .map(|descriptor| prepare_descriptor(descriptor, &limits))
            .collect()
    }

    #[test]
    fn namespace_is_stable_bounded_and_independent_of_display_name() {
        let id = McpServerId::from_uuid(
            uuid::Uuid::parse_str("12345678-1234-4234-8234-123456789abc").unwrap(),
        );
        let first = build_model_name(id, "Unicode / VERY-LONG.tool-name", 64);
        let second = build_model_name(id, "Unicode / VERY-LONG.tool-name", 64);
        assert_eq!(first, second);
        assert!(first.len() <= 64);
        assert!(first
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_lowercase() || byte.is_ascii_digit()));
    }

    #[test]
    fn normalized_collisions_receive_stable_distinct_names_and_diagnostics() {
        let id = McpServerId::new();
        let policy = McpCatalogPolicy::default();
        let (tools, diagnostics) = build_tools(
            id,
            candidates(vec![descriptor("alpha.beta"), descriptor("alpha/beta")]),
            &policy,
        )
        .unwrap();
        assert_ne!(tools[0].model_name, tools[1].model_name);
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == McpCatalogDiagnosticKind::NormalizationCollision
        }));
    }

    #[test]
    fn duplicate_raw_names_are_retained_as_an_explicit_unroutable_diagnostic() {
        let id = McpServerId::new();
        let (tools, diagnostics) = build_tools(
            id,
            candidates(vec![descriptor("same"), descriptor("same")]),
            &McpCatalogPolicy::default(),
        )
        .unwrap();
        assert_eq!(tools.len(), 1);
        assert!(!tools[0].routable);
        assert_eq!(diagnostics[0].occurrence_count, 2);
    }

    #[tokio::test]
    async fn discovers_every_page_and_preserves_opaque_cursor_routing() {
        let id = McpServerId::new();
        let peer = ScriptedPeer::new(
            id,
            vec![
                (None, Ok(page(vec![descriptor("first")], Some("opaque-1")))),
                (Some("opaque-1"), Ok(page(vec![descriptor("second")], None))),
            ],
        );
        let snapshot = discover_catalog(peer.as_ref(), id, None, &McpCatalogPolicy::default())
            .await
            .unwrap();
        assert_eq!(snapshot.completeness, McpCatalogCompleteness::Complete);
        assert_eq!(snapshot.page_count, 2);
        assert_eq!(snapshot.tools.len(), 2);
        assert_eq!(
            *peer.requested_cursors.lock().unwrap(),
            vec![None, Some("opaque-1".to_string())]
        );
    }

    #[tokio::test]
    async fn repeated_cursor_returns_explicit_partial_catalog() {
        let id = McpServerId::new();
        let peer = ScriptedPeer::new(
            id,
            vec![
                (None, Ok(page(vec![descriptor("first")], Some("again")))),
                (
                    Some("again"),
                    Ok(page(vec![descriptor("second")], Some("again"))),
                ),
            ],
        );
        let snapshot = discover_catalog(peer.as_ref(), id, None, &McpCatalogPolicy::default())
            .await
            .unwrap();
        assert_eq!(
            snapshot.completeness,
            McpCatalogCompleteness::Partial(McpCatalogIssue::RepeatedCursor)
        );
        assert_eq!(snapshot.tools.len(), 2);
        assert_eq!(snapshot.generation, 0);
    }

    #[tokio::test]
    async fn page_and_tool_limits_fail_closed_without_request_storms() {
        let page_limited_id = McpServerId::new();
        let page_limited = ScriptedPeer::new(
            page_limited_id,
            vec![(None, Ok(page(vec![descriptor("first")], Some("more"))))],
        );
        let mut page_policy = McpCatalogPolicy::default();
        page_policy.limits.max_pages = 1;
        let page_snapshot =
            discover_catalog(page_limited.as_ref(), page_limited_id, None, &page_policy)
                .await
                .unwrap();
        assert_eq!(
            page_snapshot.completeness,
            McpCatalogCompleteness::Partial(McpCatalogIssue::PageLimitExceeded)
        );
        assert_eq!(page_limited.requested_cursors.lock().unwrap().len(), 1);

        let tool_limited_id = McpServerId::new();
        let tool_limited = ScriptedPeer::new(
            tool_limited_id,
            vec![(
                None,
                Ok(page(vec![descriptor("first"), descriptor("second")], None)),
            )],
        );
        let mut tool_policy = McpCatalogPolicy::default();
        tool_policy.limits.max_tools = 1;
        let tool_snapshot =
            discover_catalog(tool_limited.as_ref(), tool_limited_id, None, &tool_policy)
                .await
                .unwrap();
        assert_eq!(
            tool_snapshot.completeness,
            McpCatalogCompleteness::Failed(McpCatalogIssue::ToolLimitExceeded)
        );
        assert!(tool_snapshot.tools.is_empty());
    }

    #[tokio::test]
    async fn generation_changes_only_when_effective_valid_content_changes() {
        let id = McpServerId::new();
        let first_peer = ScriptedPeer::new(
            id,
            vec![(
                None,
                Ok(page(vec![descriptor("alpha"), descriptor("beta")], None)),
            )],
        );
        let first = discover_catalog(first_peer.as_ref(), id, None, &McpCatalogPolicy::default())
            .await
            .unwrap();
        assert_eq!(first.generation, 1);

        let mut alpha_reordered = descriptor("alpha");
        alpha_reordered.input_schema =
            json!({"properties": {"value": {"type": "string"}}, "type": "object"});
        let mut first_alpha = descriptor("alpha");
        first_alpha.input_schema =
            json!({"type": "object", "properties": {"value": {"type": "string"}}});
        let baseline_peer = ScriptedPeer::new(
            id,
            vec![(None, Ok(page(vec![first_alpha, descriptor("beta")], None)))],
        );
        let baseline = discover_catalog(
            baseline_peer.as_ref(),
            id,
            Some(&first),
            &McpCatalogPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(baseline.generation, 2);

        let same_peer = ScriptedPeer::new(
            id,
            vec![(
                None,
                Ok(page(vec![descriptor("beta"), alpha_reordered], None)),
            )],
        );
        let same = discover_catalog(
            same_peer.as_ref(),
            id,
            Some(&baseline),
            &McpCatalogPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(same.generation, baseline.generation);

        let changed_peer = ScriptedPeer::new(
            id,
            vec![(
                None,
                Ok(page(vec![descriptor("beta"), descriptor("gamma")], None)),
            )],
        );
        let changed = discover_catalog(
            changed_peer.as_ref(),
            id,
            Some(&same),
            &McpCatalogPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(changed.generation, same.generation + 1);
    }

    #[tokio::test]
    async fn failed_refresh_retains_last_valid_catalog_as_stale() {
        let id = McpServerId::new();
        let initial_peer =
            ScriptedPeer::new(id, vec![(None, Ok(page(vec![descriptor("stable")], None)))]);
        let initial = discover_catalog(
            initial_peer.as_ref(),
            id,
            None,
            &McpCatalogPolicy::default(),
        )
        .await
        .unwrap();
        let failing_peer = ScriptedPeer::new(
            id,
            vec![(None, Err(McpError::protocol("peer detail must not escape")))],
        );
        let stale = discover_catalog(
            failing_peer.as_ref(),
            id,
            Some(&initial),
            &McpCatalogPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            stale.completeness,
            McpCatalogCompleteness::Stale(McpCatalogIssue::RequestFailed)
        );
        assert_eq!(stale.generation, initial.generation);
        assert_eq!(stale.tools, initial.tools);
    }

    #[tokio::test]
    async fn server_without_tools_capability_has_complete_empty_catalog_without_request() {
        let id = McpServerId::new();
        let mut peer = ScriptedPeer::new(id, Vec::new());
        Arc::get_mut(&mut peer)
            .expect("unshared scripted peer")
            .protocol
            .capabilities
            .tools = false;
        let snapshot = discover_catalog(peer.as_ref(), id, None, &McpCatalogPolicy::default())
            .await
            .unwrap();
        assert_eq!(snapshot.completeness, McpCatalogCompleteness::Complete);
        assert_eq!(snapshot.generation, 1);
        assert!(snapshot.tools.is_empty());
        assert!(peer.requested_cursors.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn schema_and_full_descriptor_limits_are_independent_and_fail_closed() {
        let schema_id = McpServerId::new();
        let mut schema_heavy = descriptor("schema_heavy");
        schema_heavy.input_schema = json!({"const": "x".repeat(256)});
        let schema_peer =
            ScriptedPeer::new(schema_id, vec![(None, Ok(page(vec![schema_heavy], None)))]);
        let mut schema_policy = McpCatalogPolicy::default();
        schema_policy.limits.max_schema_bytes_per_page = 32;
        let schema_snapshot =
            discover_catalog(schema_peer.as_ref(), schema_id, None, &schema_policy)
                .await
                .unwrap();
        assert_eq!(
            schema_snapshot.completeness,
            McpCatalogCompleteness::Failed(McpCatalogIssue::PageSchemaLimitExceeded)
        );

        let descriptor_id = McpServerId::new();
        let mut descriptor_heavy = descriptor("descriptor_heavy");
        descriptor_heavy.description = Some("x".repeat(256));
        let descriptor_peer = ScriptedPeer::new(
            descriptor_id,
            vec![(None, Ok(page(vec![descriptor_heavy], None)))],
        );
        let mut descriptor_policy = McpCatalogPolicy::default();
        descriptor_policy.limits.max_descriptor_bytes_per_page = 64;
        let descriptor_snapshot = discover_catalog(
            descriptor_peer.as_ref(),
            descriptor_id,
            None,
            &descriptor_policy,
        )
        .await
        .unwrap();
        assert_eq!(
            descriptor_snapshot.completeness,
            McpCatalogCompleteness::Failed(McpCatalogIssue::PageDescriptorLimitExceeded)
        );
    }

    #[tokio::test]
    async fn invalid_untrusted_descriptors_are_unroutable_without_hiding_healthy_tools() {
        let limits = McpSecurityLimits::default();
        let mut too_deep = json!({"type": "string"});
        for _ in 0..limits.max_schema_depth {
            too_deep = json!({"not": too_deep});
        }
        let too_many_nodes = json!({
            "anyOf": vec![Value::Bool(true); limits.max_schema_nodes]
        });
        let properties = (0..=limits.max_schema_properties)
            .map(|index| (format!("property_{index}"), json!({"type": "string"})))
            .collect::<Map<_, _>>();

        let mut descriptors = vec![descriptor("healthy")];
        for (name, schema) in [
            ("too_deep", too_deep),
            ("too_many_nodes", too_many_nodes),
            (
                "too_many_properties",
                json!({"type": "object", "properties": properties}),
            ),
            (
                "too_many_enum_values",
                json!({"type": "string", "enum": vec!["x"; limits.max_schema_enum_values + 1]}),
            ),
            (
                "remote_ref",
                json!({"type": "object", "$ref": "https://example.invalid/schema.json"}),
            ),
            (
                "oversized_literal",
                json!({"type": "string", "const": "x".repeat(limits.max_schema_string_literal_bytes + 1)}),
            ),
        ] {
            let mut invalid = descriptor(name);
            invalid.input_schema = schema;
            descriptors.push(invalid);
        }
        let mut oversized_description = descriptor("oversized_description");
        oversized_description.description = Some("x".repeat(limits.max_tool_description_bytes + 1));
        descriptors.push(oversized_description);
        descriptors.push(descriptor(&"x".repeat(limits.max_raw_tool_name_bytes + 1)));

        let id = McpServerId::new();
        let peer = ScriptedPeer::new(id, vec![(None, Ok(page(descriptors, None)))]);
        let snapshot = discover_catalog(peer.as_ref(), id, None, &McpCatalogPolicy::default())
            .await
            .unwrap();

        assert_eq!(snapshot.completeness, McpCatalogCompleteness::Complete);
        assert!(snapshot
            .tools
            .iter()
            .find(|tool| tool.raw_name == "healthy")
            .is_some_and(|tool| tool.routable));
        assert_eq!(
            snapshot.tools.iter().filter(|tool| !tool.routable).count(),
            8
        );
        assert_eq!(
            snapshot
                .diagnostics
                .iter()
                .filter(|diagnostic| { diagnostic.kind == McpCatalogDiagnosticKind::InvalidSchema })
                .count(),
            6
        );
        assert!(snapshot.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == McpCatalogDiagnosticKind::DescriptionLimitExceeded
        }));
        assert!(snapshot
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.kind == McpCatalogDiagnosticKind::InvalidRawName }));
    }

    #[test]
    fn provider_facing_model_name_limit_cannot_exceed_sixty_four_bytes() {
        let limits = McpCatalogLimits {
            max_model_name_bytes: 65,
            ..McpCatalogLimits::default()
        };
        assert_eq!(
            limits.validate().unwrap_err().kind,
            crate::McpErrorKind::Config
        );
    }
}
