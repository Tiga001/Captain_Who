use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    McpContentBlock, McpDispatchCertainty, McpEmbeddedResource, McpError, McpProtocolSnapshot,
    McpToolResult,
};

const MAX_CONFIGURED_BYTE_LIMIT: usize = 64 * 1024 * 1024;
const MAX_CONFIGURED_COUNT_LIMIT: usize = 1_000_000;

/// Host-owned safety ceilings for untrusted MCP protocol metadata and content.
///
/// Transport and catalog policies may choose stricter values, but must not
/// exceed these ceilings. Keeping the limits in one stable domain type prevents
/// a future transport from silently accepting content that stdio rejects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct McpSecurityLimits {
    pub max_server_instructions_bytes: usize,
    pub max_server_implementation_name_bytes: usize,
    pub max_server_implementation_version_bytes: usize,
    pub max_capability_metadata_bytes: usize,
    pub max_capability_extensions: usize,
    pub max_capability_extension_name_bytes: usize,
    pub max_total_capability_extension_name_bytes: usize,
    pub max_raw_tool_name_bytes: usize,
    pub max_tool_description_bytes: usize,
    pub max_schema_bytes_per_tool: usize,
    pub max_schema_depth: usize,
    pub max_schema_nodes: usize,
    pub max_schema_properties: usize,
    pub max_schema_enum_values: usize,
    pub max_tools: usize,
    pub max_catalog_pages: usize,
    pub max_schema_bytes_per_page: usize,
    pub max_total_schema_bytes: usize,
    pub max_descriptor_bytes_per_page: usize,
    pub max_total_descriptor_bytes: usize,
    pub max_cursor_bytes: usize,
    pub max_model_name_bytes: usize,
    pub max_protocol_message_bytes: usize,
    pub max_raw_tool_result_bytes: usize,
    pub max_raw_arguments_bytes: usize,
    pub max_arguments_depth: usize,
    pub max_arguments_nodes: usize,
    pub max_argument_object_properties: usize,
    pub max_model_text_bytes: usize,
    pub max_structured_content_bytes: usize,
    pub max_structured_content_depth: usize,
    pub max_structured_content_nodes: usize,
    pub max_encoded_media_bytes: usize,
    pub max_total_encoded_media_bytes: usize,
    pub max_content_blocks: usize,
    pub max_schema_string_literal_bytes: usize,
    pub max_safe_error_bytes: usize,
    pub default_tool_timeout_ms: u64,
    pub max_tool_timeout_ms: u64,
    pub max_stderr_line_bytes: usize,
    pub max_stderr_retained_bytes: usize,
    pub stderr_rate_limit_bytes_per_second: usize,
}

impl Default for McpSecurityLimits {
    fn default() -> Self {
        Self {
            max_server_instructions_bytes: 32 * 1024,
            max_server_implementation_name_bytes: 256,
            max_server_implementation_version_bytes: 128,
            max_capability_metadata_bytes: 64 * 1024,
            max_capability_extensions: 64,
            max_capability_extension_name_bytes: 256,
            max_total_capability_extension_name_bytes: 16 * 1024,
            max_raw_tool_name_bytes: 1_024,
            max_tool_description_bytes: 16 * 1024,
            max_schema_bytes_per_tool: 256 * 1024,
            max_schema_depth: 32,
            max_schema_nodes: 4_096,
            max_schema_properties: 256,
            max_schema_enum_values: 256,
            max_tools: 1_024,
            max_catalog_pages: 32,
            max_schema_bytes_per_page: 1024 * 1024,
            max_total_schema_bytes: 8 * 1024 * 1024,
            max_descriptor_bytes_per_page: 2 * 1024 * 1024,
            max_total_descriptor_bytes: 16 * 1024 * 1024,
            max_cursor_bytes: 4 * 1024,
            max_model_name_bytes: 64,
            max_protocol_message_bytes: 8 * 1024 * 1024,
            max_raw_tool_result_bytes: 4 * 1024 * 1024,
            max_raw_arguments_bytes: 64 * 1024,
            max_arguments_depth: 32,
            max_arguments_nodes: 4_096,
            max_argument_object_properties: 256,
            max_model_text_bytes: 16 * 1024,
            max_structured_content_bytes: 8 * 1024,
            max_structured_content_depth: 32,
            max_structured_content_nodes: 4_096,
            max_encoded_media_bytes: 1024 * 1024,
            max_total_encoded_media_bytes: 2 * 1024 * 1024,
            max_content_blocks: 128,
            max_schema_string_literal_bytes: 4 * 1024,
            max_safe_error_bytes: 4 * 1024,
            default_tool_timeout_ms: 60_000,
            max_tool_timeout_ms: 300_000,
            max_stderr_line_bytes: 8 * 1024,
            max_stderr_retained_bytes: 64 * 1024,
            stderr_rate_limit_bytes_per_second: 16 * 1024,
        }
    }
}

impl McpSecurityLimits {
    pub fn validate(&self) -> Result<(), McpError> {
        let byte_limits = [
            self.max_server_instructions_bytes,
            self.max_server_implementation_name_bytes,
            self.max_server_implementation_version_bytes,
            self.max_capability_metadata_bytes,
            self.max_capability_extension_name_bytes,
            self.max_total_capability_extension_name_bytes,
            self.max_raw_tool_name_bytes,
            self.max_tool_description_bytes,
            self.max_schema_bytes_per_tool,
            self.max_schema_bytes_per_page,
            self.max_total_schema_bytes,
            self.max_descriptor_bytes_per_page,
            self.max_total_descriptor_bytes,
            self.max_cursor_bytes,
            self.max_protocol_message_bytes,
            self.max_raw_tool_result_bytes,
            self.max_raw_arguments_bytes,
            self.max_model_text_bytes,
            self.max_structured_content_bytes,
            self.max_encoded_media_bytes,
            self.max_total_encoded_media_bytes,
            self.max_schema_string_literal_bytes,
            self.max_safe_error_bytes,
            self.max_stderr_line_bytes,
            self.max_stderr_retained_bytes,
            self.stderr_rate_limit_bytes_per_second,
        ];
        if byte_limits
            .into_iter()
            .any(|limit| limit == 0 || limit > MAX_CONFIGURED_BYTE_LIMIT)
        {
            return Err(McpError::config("MCP security byte limits are invalid"));
        }
        let count_limits = [
            self.max_schema_depth,
            self.max_schema_nodes,
            self.max_schema_properties,
            self.max_schema_enum_values,
            self.max_capability_extensions,
            self.max_tools,
            self.max_catalog_pages,
            self.max_arguments_depth,
            self.max_arguments_nodes,
            self.max_argument_object_properties,
            self.max_structured_content_depth,
            self.max_structured_content_nodes,
            self.max_content_blocks,
        ];
        if count_limits
            .into_iter()
            .any(|limit| limit == 0 || limit > MAX_CONFIGURED_COUNT_LIMIT)
            || !(53..=64).contains(&self.max_model_name_bytes)
            || self.max_schema_bytes_per_tool > self.max_schema_bytes_per_page
            || self.max_capability_extension_name_bytes
                > self.max_total_capability_extension_name_bytes
            || self.max_total_capability_extension_name_bytes > self.max_capability_metadata_bytes
            || self.max_schema_bytes_per_page > self.max_total_schema_bytes
            || self.max_schema_properties > self.max_schema_nodes
            || self.max_schema_enum_values > self.max_schema_nodes
            || self.max_descriptor_bytes_per_page > self.max_total_descriptor_bytes
            || self.max_raw_tool_result_bytes > self.max_protocol_message_bytes
            || self.max_raw_arguments_bytes > self.max_protocol_message_bytes
            || self.max_arguments_depth > self.max_arguments_nodes
            || self.max_argument_object_properties > self.max_arguments_nodes
            || self.max_encoded_media_bytes > self.max_raw_tool_result_bytes
            || self.max_encoded_media_bytes > self.max_total_encoded_media_bytes
            || self.max_total_encoded_media_bytes > self.max_raw_tool_result_bytes
            || self.default_tool_timeout_ms == 0
            || self.default_tool_timeout_ms > self.max_tool_timeout_ms
            || self.max_tool_timeout_ms > 300_000
            || self.max_stderr_line_bytes > self.max_stderr_retained_bytes
        {
            return Err(McpError::config("MCP security limits are inconsistent"));
        }
        Ok(())
    }

    /// Validates the bounded, public protocol projection accepted from any
    /// connector before it can enter Manager state or future IPC DTOs.
    pub fn validate_protocol_snapshot(
        &self,
        snapshot: &McpProtocolSnapshot,
    ) -> Result<(), McpError> {
        if !matches!(
            snapshot.negotiated_version.as_str(),
            "2026-07-28" | "2025-11-25"
        ) {
            return Err(McpError::negotiation(
                "MCP server selected an unsupported protocol version",
            ));
        }
        if let Some(server) = &snapshot.server {
            validate_protocol_label(
                &server.name,
                self.max_server_implementation_name_bytes,
                "MCP server implementation name",
            )?;
            validate_protocol_label(
                &server.version,
                self.max_server_implementation_version_bytes,
                "MCP server implementation version",
            )?;
        }
        if snapshot.capabilities.extensions.len() > self.max_capability_extensions {
            return Err(McpError::negotiation(
                "MCP server declared too many capability extensions",
            ));
        }
        let mut total_extension_name_bytes = 0usize;
        for extension in &snapshot.capabilities.extensions {
            validate_protocol_label(
                extension,
                self.max_capability_extension_name_bytes,
                "MCP capability extension name",
            )?;
            total_extension_name_bytes = total_extension_name_bytes.saturating_add(extension.len());
            if total_extension_name_bytes > self.max_total_capability_extension_name_bytes {
                return Err(McpError::negotiation(
                    "MCP capability extension names exceeded the aggregate size limit",
                ));
            }
        }
        Ok(())
    }
}

fn validate_protocol_label(
    value: &str,
    max_bytes: usize,
    label: &'static str,
) -> Result<(), McpError> {
    if value.is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(McpError::negotiation(format!(
            "{label} was empty, oversized, or contained control characters"
        )));
    }
    Ok(())
}

pub(crate) fn validate_schema_value(
    schema: &Value,
    limits: &McpSecurityLimits,
) -> Result<(), McpError> {
    if !schema.is_object() {
        return Err(McpError::protocol("MCP tool schema must be a JSON object"));
    }

    let mut stack = vec![(schema, 1_usize)];
    let mut nodes = 0_usize;
    while let Some((value, depth)) = stack.pop() {
        if depth > limits.max_schema_depth {
            return Err(McpError::protocol(
                "MCP tool schema exceeded the configured depth limit",
            ));
        }
        nodes = nodes
            .checked_add(1)
            .ok_or_else(|| McpError::protocol("MCP tool schema node count overflowed"))?;
        if nodes > limits.max_schema_nodes {
            return Err(McpError::protocol(
                "MCP tool schema exceeded the configured node limit",
            ));
        }

        match value {
            Value::String(value) => {
                if value.len() > limits.max_schema_string_literal_bytes {
                    return Err(McpError::protocol(
                        "MCP tool schema contained an oversized string literal",
                    ));
                }
            }
            Value::Array(values) => {
                for child in values.iter().rev() {
                    stack.push((child, depth.saturating_add(1)));
                }
            }
            Value::Object(object) => {
                if let Some(properties) = object.get("properties") {
                    let Some(properties) = properties.as_object() else {
                        return Err(McpError::protocol(
                            "MCP tool schema properties must be a JSON object",
                        ));
                    };
                    if properties.len() > limits.max_schema_properties {
                        return Err(McpError::protocol(
                            "MCP tool schema exceeded the configured property limit",
                        ));
                    }
                }
                if let Some(values) = object.get("enum") {
                    let Some(values) = values.as_array() else {
                        return Err(McpError::protocol(
                            "MCP tool schema enum must be a JSON array",
                        ));
                    };
                    if values.len() > limits.max_schema_enum_values {
                        return Err(McpError::protocol(
                            "MCP tool schema exceeded the configured enum limit",
                        ));
                    }
                }
                for keyword in ["$ref", "$dynamicRef", "$recursiveRef"] {
                    if let Some(reference) = object.get(keyword) {
                        let Some(reference) = reference.as_str() else {
                            return Err(McpError::protocol(
                                "MCP tool schema reference must be a string",
                            ));
                        };
                        if !is_safe_local_schema_reference(reference) {
                            return Err(McpError::protocol(
                                "MCP tool schema contains a non-local or unsafe reference",
                            ));
                        }
                    }
                }
                for child in object.values().rev() {
                    stack.push((child, depth.saturating_add(1)));
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }

    let encoded = serde_json::to_vec(schema)
        .map_err(|_| McpError::protocol("MCP tool schema could not be measured safely"))?;
    if encoded.len() > limits.max_schema_bytes_per_tool {
        return Err(McpError::protocol(
            "MCP tool schema exceeded the configured byte limit",
        ));
    }
    Ok(())
}

pub(crate) fn validate_tool_arguments(
    arguments: &Value,
    limits: &McpSecurityLimits,
) -> Result<(), McpError> {
    if !arguments.is_object() {
        return Err(McpError::config("MCP tool arguments must be a JSON object"));
    }
    validate_json_structure(
        arguments,
        limits.max_arguments_depth,
        limits.max_arguments_nodes,
        Some(limits.max_argument_object_properties),
        "MCP tool arguments",
        McpError::config,
    )?;
    let encoded = serde_json::to_vec(arguments)
        .map_err(|_| McpError::config("MCP tool arguments could not be measured safely"))?;
    if encoded.len() > limits.max_raw_arguments_bytes {
        return Err(McpError::config(
            "MCP tool arguments exceeded the configured byte limit",
        ));
    }
    Ok(())
}

pub(crate) fn validate_structured_content(
    structured: &Value,
    limits: &McpSecurityLimits,
) -> Result<(), McpError> {
    validate_json_structure(
        structured,
        limits.max_structured_content_depth,
        limits.max_structured_content_nodes,
        None,
        "MCP structured result",
        McpError::protocol,
    )?;
    let encoded = serde_json::to_vec(structured)
        .map_err(|_| McpError::protocol("MCP structured result could not be measured safely"))?;
    if encoded.len() > limits.max_structured_content_bytes {
        return Err(McpError::protocol(
            "MCP structured result exceeded the configured byte limit",
        ));
    }
    Ok(())
}

pub(crate) fn validate_tool_result(
    result: &McpToolResult,
    limits: &McpSecurityLimits,
) -> Result<(), McpError> {
    if result.content.len() > limits.max_content_blocks {
        return Err(McpError::output_too_large(
            "MCP tools/call",
            "MCP tool result exceeded the configured content-block limit",
        ));
    }
    if let Some(structured) = &result.structured_content {
        if validate_structured_content(structured, limits).is_err() {
            return Err(McpError::output_too_large(
                "MCP tools/call",
                "MCP structured result exceeded the configured safety budget",
            ));
        }
    }
    let encoded = serde_json::to_vec(result).map_err(|_| {
        McpError::protocol("MCP tool result could not be measured safely")
            .with_dispatch_certainty(McpDispatchCertainty::ResponseReceived)
    })?;
    if encoded.len() > limits.max_raw_tool_result_bytes {
        return Err(McpError::output_too_large(
            "MCP tools/call",
            "MCP tool result exceeded the configured byte limit",
        ));
    }
    let mut total_media = 0_usize;
    for block in &result.content {
        let media_bytes = match block {
            McpContentBlock::Image { data, .. } | McpContentBlock::Audio { data, .. } => data.len(),
            McpContentBlock::EmbeddedResource {
                resource: McpEmbeddedResource::Blob { data, .. },
            } => data.len(),
            _ => 0,
        };
        if media_bytes > limits.max_encoded_media_bytes {
            return Err(McpError::output_too_large(
                "MCP tools/call",
                "MCP tool result contained an oversized encoded media block",
            ));
        }
        total_media = total_media.checked_add(media_bytes).ok_or_else(|| {
            McpError::output_too_large(
                "MCP tools/call",
                "MCP tool result media byte count overflowed",
            )
        })?;
    }
    if total_media > limits.max_total_encoded_media_bytes {
        return Err(McpError::output_too_large(
            "MCP tools/call",
            "MCP tool result exceeded the aggregate encoded media limit",
        ));
    }
    Ok(())
}

fn validate_json_structure(
    value: &Value,
    max_depth: usize,
    max_nodes: usize,
    max_object_properties: Option<usize>,
    label: &'static str,
    error: fn(String) -> McpError,
) -> Result<(), McpError> {
    let mut stack = vec![(value, 1_usize)];
    let mut nodes = 0_usize;
    while let Some((value, depth)) = stack.pop() {
        if depth > max_depth {
            return Err(error(format!(
                "{label} exceeded the configured depth limit"
            )));
        }
        nodes = nodes
            .checked_add(1)
            .ok_or_else(|| error(format!("{label} node count overflowed")))?;
        if nodes > max_nodes {
            return Err(error(format!("{label} exceeded the configured node limit")));
        }
        match value {
            Value::Array(values) => {
                for child in values.iter().rev() {
                    stack.push((child, depth.saturating_add(1)));
                }
            }
            Value::Object(object) => {
                if max_object_properties.is_some_and(|limit| object.len() > limit) {
                    return Err(error(format!(
                        "{label} exceeded the configured object-property limit"
                    )));
                }
                for child in object.values().rev() {
                    stack.push((child, depth.saturating_add(1)));
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    Ok(())
}

fn is_safe_local_schema_reference(reference: &str) -> bool {
    if reference == "#" {
        return true;
    }
    let Some(fragment) = reference.strip_prefix('#') else {
        return false;
    };
    if fragment.is_empty()
        || fragment
            .chars()
            .any(|character| character.is_control() || matches!(character, '%' | '\\' | '#'))
    {
        return false;
    }
    if let Some(pointer) = fragment.strip_prefix('/') {
        let mut bytes = pointer.bytes();
        while let Some(byte) = bytes.next() {
            if byte == b'~' && !matches!(bytes.next(), Some(b'0' | b'1')) {
                return false;
            }
        }
        true
    } else {
        fragment.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric()
                || (index > 0 && matches!(byte, b'_' | b'-' | b'.' | b':'))
                || (index == 0 && matches!(byte, b'_' | b':'))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map};

    #[test]
    fn defaults_are_internally_consistent() {
        McpSecurityLimits::default().validate().unwrap();
    }

    #[test]
    fn inconsistent_limits_fail_closed() {
        let mut limits = McpSecurityLimits::default();
        limits.max_encoded_media_bytes = limits.max_raw_tool_result_bytes + 1;
        assert!(limits.validate().is_err());
    }

    #[test]
    fn public_protocol_snapshot_rejects_unbounded_or_ambiguous_peer_metadata() {
        let limits = McpSecurityLimits::default();
        let base = McpProtocolSnapshot {
            negotiated_version: "2026-07-28".to_string(),
            lifecycle: crate::McpLifecycleKind::Discover,
            server: Some(crate::McpImplementationInfo {
                name: "owned-fixture".to_string(),
                version: "1.0.0".to_string(),
            }),
            capabilities: crate::McpCapabilitySnapshot::default(),
        };
        limits.validate_protocol_snapshot(&base).unwrap();

        let mut unknown_version = base.clone();
        unknown_version.negotiated_version = "future-or-peer-controlled".to_string();
        assert!(limits.validate_protocol_snapshot(&unknown_version).is_err());

        let mut oversized_name = base.clone();
        oversized_name.server.as_mut().unwrap().name =
            "n".repeat(limits.max_server_implementation_name_bytes + 1);
        assert!(limits.validate_protocol_snapshot(&oversized_name).is_err());

        let mut controlled_version = base.clone();
        controlled_version.server.as_mut().unwrap().version = "1.0\nforged".to_string();
        assert!(limits
            .validate_protocol_snapshot(&controlled_version)
            .is_err());

        let mut too_many_extensions = base;
        too_many_extensions.capabilities.extensions =
            vec!["io.example.extension".to_string(); limits.max_capability_extensions + 1];
        assert!(limits
            .validate_protocol_snapshot(&too_many_extensions)
            .is_err());
    }

    #[test]
    fn schema_structure_limits_and_local_references_fail_closed() {
        let limits = McpSecurityLimits::default();

        let mut too_deep = json!({"type": "string"});
        for _ in 0..limits.max_schema_depth {
            too_deep = json!({"not": too_deep});
        }
        assert!(validate_schema_value(&too_deep, &limits).is_err());

        let too_many_nodes = json!({
            "anyOf": vec![Value::Bool(true); limits.max_schema_nodes]
        });
        assert!(validate_schema_value(&too_many_nodes, &limits).is_err());

        let properties = (0..=limits.max_schema_properties)
            .map(|index| (format!("property_{index}"), json!({"type": "string"})))
            .collect::<Map<_, _>>();
        assert!(validate_schema_value(
            &json!({"type": "object", "properties": properties}),
            &limits,
        )
        .is_err());

        assert!(validate_schema_value(
            &json!({"type": "string", "enum": vec!["x"; limits.max_schema_enum_values + 1]}),
            &limits,
        )
        .is_err());
        assert!(validate_schema_value(
            &json!({"type": "object", "$ref": "https://example.invalid/schema.json"}),
            &limits,
        )
        .is_err());
        assert!(validate_schema_value(
            &json!({"type": "object", "$ref": "#/$defs/safe~1name"}),
            &limits,
        )
        .is_ok());
        assert!(validate_schema_value(
            &json!({"type": "string", "const": "x".repeat(limits.max_schema_string_literal_bytes + 1)}),
            &limits,
        )
        .is_err());
    }

    #[test]
    fn arguments_require_an_object_and_enforce_depth_nodes_and_bytes() {
        let limits = McpSecurityLimits::default();
        assert!(validate_tool_arguments(&json!(null), &limits).is_err());

        let mut too_deep = json!({});
        for _ in 0..limits.max_arguments_depth {
            too_deep = json!({"nested": too_deep});
        }
        assert!(validate_tool_arguments(&too_deep, &limits).is_err());

        let too_many_nodes = json!({
            "items": vec![Value::Null; limits.max_arguments_nodes]
        });
        assert!(validate_tool_arguments(&too_many_nodes, &limits).is_err());
        assert!(validate_tool_arguments(
            &json!({"value": "x".repeat(limits.max_raw_arguments_bytes)}),
            &limits,
        )
        .is_err());
    }

    #[test]
    fn arguments_reject_oversized_nested_objects_before_encoding() {
        let limits = McpSecurityLimits::default();
        let allowed = (0..limits.max_argument_object_properties)
            .map(|index| (format!("p{index}"), Value::Null))
            .collect::<Map<_, _>>();
        assert!(validate_tool_arguments(&json!({"nested": allowed}), &limits).is_ok());

        let oversized = (0..=limits.max_argument_object_properties)
            .map(|index| (format!("p{index}"), Value::Null))
            .collect::<Map<_, _>>();
        let error = validate_tool_arguments(&json!({"nested": oversized}), &limits).unwrap_err();
        assert_eq!(error.kind, crate::McpErrorKind::Config);
        assert!(error.message.contains("object-property limit"));
    }

    #[test]
    fn structured_results_enforce_depth_and_nodes_before_encoding() {
        let limits = McpSecurityLimits::default();
        let mut too_deep = json!(null);
        for _ in 0..limits.max_structured_content_depth {
            too_deep = json!({"nested": too_deep});
        }
        assert!(validate_structured_content(&too_deep, &limits).is_err());

        let too_many_nodes = Value::Array(vec![Value::Null; limits.max_structured_content_nodes]);
        assert!(validate_structured_content(&too_many_nodes, &limits).is_err());
    }
}
