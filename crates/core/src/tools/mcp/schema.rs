//! Provider-schema normalization and provenance security validation.

use super::*;

pub(super) fn normalize_server_display_name(
    display_name: &str,
    provenance: &AgentMcpToolProvenance,
) -> String {
    let normalized = display_name
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let normalized = truncate_utf8(normalized.trim(), MAX_MCP_SERVER_DISPLAY_NAME_BYTES);
    if normalized.is_empty() {
        let suffix = provenance.server_id.get(..8).unwrap_or("unknown");
        format!("MCP server {suffix}")
    } else {
        normalized
    }
}

pub(super) fn mcp_tool_risk(annotations: &McpAgentToolAnnotations) -> AgentMcpToolRisk {
    if annotations.destructive_hint == Some(true) {
        AgentMcpToolRisk::DestructiveClaimed
    } else if annotations.open_world_hint == Some(true) {
        AgentMcpToolRisk::OpenWorldClaimed
    } else if annotations.read_only_hint == Some(true) {
        AgentMcpToolRisk::ReadOnlyClaimed
    } else if annotations.idempotent_hint == Some(false) {
        AgentMcpToolRisk::SideEffectsPossible
    } else {
        AgentMcpToolRisk::Unknown
    }
}

pub(super) fn validate_catalog_provenance(
    provenance: &AgentMcpToolProvenance,
) -> Result<(), McpToolRegistrationDiagnostic> {
    if uuid::Uuid::parse_str(&provenance.server_id).is_err()
        || provenance.raw_tool_name.trim().is_empty()
        || provenance.raw_tool_name.trim() != provenance.raw_tool_name
        || provenance.raw_tool_name.len() > MAX_MCP_RAW_TOOL_NAME_BYTES
        || provenance.raw_tool_name.chars().any(char::is_control)
        || !valid_canonical_uuid_v4(&provenance.config_epoch)
        || provenance.registry_revision == 0
        || provenance.catalog_generation == 0
        || !valid_digest(&provenance.config_digest)
        || !valid_digest(&provenance.catalog_digest)
        || !valid_digest(&provenance.catalog_schema_digest)
        || !valid_scope(&provenance.scope)
    {
        return Err(McpToolRegistrationDiagnostic::for_tool(
            provenance,
            McpToolDiagnosticCode::InvalidIdentity,
        ));
    }
    if !valid_model_name(&provenance.model_tool_name) {
        return Err(McpToolRegistrationDiagnostic::for_tool(
            provenance,
            McpToolDiagnosticCode::InvalidModelName,
        ));
    }
    Ok(())
}

pub(super) fn validate_provenance(
    provenance: &AgentMcpToolProvenance,
) -> Result<(), McpToolRegistrationDiagnostic> {
    validate_catalog_provenance(provenance)?;
    if !valid_digest(&provenance.schema_digest)
        || provenance.schema_normalizer_version != MCP_INPUT_SCHEMA_NORMALIZER_VERSION
    {
        return Err(McpToolRegistrationDiagnostic::for_tool(
            provenance,
            McpToolDiagnosticCode::InvalidIdentity,
        ));
    }
    Ok(())
}

fn valid_scope(scope: &AgentMcpServerScope) -> bool {
    let identifier = match scope {
        AgentMcpServerScope::Project { project_id } => Some(project_id),
        AgentMcpServerScope::Plugin { plugin_id } => Some(plugin_id),
        AgentMcpServerScope::Builtin | AgentMcpServerScope::User | AgentMcpServerScope::Managed => {
            None
        }
    };
    identifier.is_none_or(|identifier| {
        !identifier.trim().is_empty()
            && identifier.trim() == identifier
            && identifier.len() <= 1_024
            && !identifier.chars().any(char::is_control)
    })
}

pub(super) fn valid_model_name(name: &str) -> bool {
    name.starts_with("mcp__")
        && !name.is_empty()
        && name.len() <= MAX_MCP_MODEL_TOOL_NAME_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub(super) fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn valid_canonical_uuid_v4(value: &str) -> bool {
    let Ok(uuid) = uuid::Uuid::parse_str(value) else {
        return false;
    };
    !uuid.is_nil() && uuid.get_version() == Some(uuid::Version::Random) && value == uuid.to_string()
}

pub(super) fn normalize_description(description: Option<&str>) -> String {
    let description = description
        .unwrap_or_default()
        .chars()
        .map(|character| {
            if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let description = description.trim();
    if description.is_empty() {
        truncate_utf8(
            "External MCP tool with no server description.",
            MAX_MCP_DESCRIPTION_BYTES,
        )
    } else {
        let body_budget = MAX_MCP_DESCRIPTION_BYTES
            .saturating_sub(MCP_DESCRIPTION_PREFIX.len())
            .saturating_sub(MCP_DESCRIPTION_TRUNCATION_MARKER.len());
        let truncated = description.len() > body_budget;
        let mut normalized = String::with_capacity(MAX_MCP_DESCRIPTION_BYTES);
        normalized.push_str(MCP_DESCRIPTION_PREFIX);
        normalized.push_str(&truncate_utf8(description, body_budget));
        if truncated {
            normalized.push_str(MCP_DESCRIPTION_TRUNCATION_MARKER);
        }
        normalized
    }
}

pub(super) fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value[..end].to_string()
}

pub(super) fn normalize_input_schema(
    provenance: &AgentMcpToolProvenance,
    schema: Value,
) -> Result<Value, McpToolRegistrationDiagnostic> {
    normalize_input_schema_value(&provenance.model_tool_name, schema).map_err(|error| {
        let code = if error.code() == Some("mcp.input_schema_too_large") {
            McpToolDiagnosticCode::SchemaTooLarge
        } else {
            McpToolDiagnosticCode::InvalidSchema
        };
        McpToolRegistrationDiagnostic::for_tool(provenance, code)
    })
}

pub(super) fn normalize_input_schema_value(
    model_tool_name: &str,
    schema: Value,
) -> AgentResult<Value> {
    let serialized = serde_json::to_vec(&schema).map_err(|_| {
        AgentError::structured(
            "mcp.invalid_input_schema",
            "The MCP input schema could not be encoded safely.",
            json!({"retryable": false}),
        )
    })?;
    if serialized.len() > MAX_MCP_PROVIDER_SCHEMA_BYTES {
        return Err(AgentError::structured(
            "mcp.input_schema_too_large",
            "The MCP input schema exceeds the Provider-facing size limit.",
            json!({"retryable": false}),
        ));
    }
    let Value::Object(mut root) = schema else {
        return Err(AgentError::structured(
            "mcp.invalid_input_schema",
            "The MCP input schema root must be an object.",
            json!({"retryable": false}),
        ));
    };
    if !root.contains_key("type") {
        root.insert("type".to_string(), Value::String("object".to_string()));
    }
    let properties = root
        .entry("properties".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            AgentError::structured(
                "mcp.invalid_input_schema",
                "The MCP input schema properties entry must be an object.",
                json!({"retryable": false}),
            )
        })?;
    if properties.contains_key(MCP_CALL_REASON_FIELD) {
        return Err(AgentError::structured(
            "mcp.invalid_input_schema",
            "The MCP input schema collides with a reserved Host field.",
            json!({"retryable": false}),
        ));
    }
    properties.insert(
        MCP_CALL_REASON_FIELD.to_string(),
        json!({
            "type": "string",
            "minLength": 1,
            "maxLength": MAX_MCP_CALL_REASON_BYTES,
            "description": "Briefly explain why this tool call is needed. Do not include argument values, credentials, or other sensitive data."
        }),
    );
    let required = root
        .entry("required".to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| {
            AgentError::structured(
                "mcp.invalid_input_schema",
                "The MCP input schema required entry must be an array.",
                json!({"retryable": false}),
            )
        })?;
    if !required
        .iter()
        .any(|entry| entry.as_str() == Some(MCP_CALL_REASON_FIELD))
    {
        required.push(Value::String(MCP_CALL_REASON_FIELD.to_string()));
    }
    let normalized = Value::Object(root);
    let normalized_bytes = serde_json::to_vec(&normalized).map_err(|_| {
        AgentError::structured(
            "mcp.invalid_input_schema",
            "The normalized MCP input schema could not be encoded safely.",
            json!({"retryable": false}),
        )
    })?;
    if normalized_bytes.len() > MAX_MCP_PROVIDER_SCHEMA_BYTES {
        return Err(AgentError::structured(
            "mcp.input_schema_too_large",
            "The normalized MCP input schema exceeds the Provider-facing size limit.",
            json!({"retryable": false}),
        ));
    }
    if schema_requests_model_supplied_credentials(&normalized) {
        return Err(AgentError::structured(
            "mcp.invalid_input_schema",
            "The MCP input schema requests model-supplied credentials.",
            json!({"retryable": false}),
        ));
    }
    validate_portable_tool_input_schema(model_tool_name, &normalized)?;
    Ok(normalized)
}

pub(super) fn normalized_input_schema_identity(
    normalized: &Value,
) -> AgentResult<McpNormalizedInputSchemaIdentity> {
    let canonical = canonical_json(normalized);
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|_| AgentError::new("MCP input schema could not be canonicalized."))?;
    let mut digest = Sha256::new();
    digest.update(MCP_PROVIDER_INPUT_SCHEMA_DIGEST_DOMAIN);
    digest.update(bytes);
    Ok(McpNormalizedInputSchemaIdentity {
        schema_digest: lower_hex(&digest.finalize()),
        normalizer_version: MCP_INPUT_SCHEMA_NORMALIZER_VERSION,
    })
}

fn schema_requests_model_supplied_credentials(schema: &Value) -> bool {
    let mut pending = vec![schema];
    let mut visited = 0_usize;
    while let Some(value) = pending.pop() {
        visited = visited.saturating_add(1);
        if visited > 4_096 {
            return true;
        }
        match value {
            Value::Object(object) => {
                if object
                    .get("properties")
                    .and_then(Value::as_object)
                    .is_some_and(|properties| {
                        properties
                            .keys()
                            .any(|name| is_sensitive_mcp_argument_name(name))
                    })
                {
                    return true;
                }
                pending.extend(object.values());
            }
            Value::Array(array) => pending.extend(array),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    false
}

fn is_sensitive_mcp_argument_name(name: &str) -> bool {
    let normalized = name
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<_>>();
    [
        b"apikey".as_slice(),
        b"apitoken".as_slice(),
        b"accesstoken".as_slice(),
        b"authtoken".as_slice(),
        b"bearertoken".as_slice(),
        b"clientsecret".as_slice(),
        b"credential".as_slice(),
        b"authorization".as_slice(),
        b"password".as_slice(),
        b"passwd".as_slice(),
        b"secret".as_slice(),
        b"sessioncookie".as_slice(),
    ]
    .iter()
    .any(|sensitive| {
        normalized
            .windows(sensitive.len())
            .any(|window| window == *sensitive)
    })
}
