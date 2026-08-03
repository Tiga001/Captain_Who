//! Provider-schema normalization and provenance security validation.

use super::*;

pub(super) fn normalize_server_display_name(
    display_name: &str,
    provenance: &AgentMcpToolProvenance,
) -> String {
    let normalized = normalize_mcp_metadata_label(display_name, MAX_MCP_SERVER_DISPLAY_NAME_BYTES);
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

pub(super) fn normalize_description(
    server_display_name: &str,
    raw_tool_name: &str,
    description: Option<&str>,
) -> String {
    let server_label = quote_mcp_metadata_label(
        server_display_name,
        MAX_MCP_SERVER_DISPLAY_NAME_BYTES,
        "MCP server",
    );
    let tool_label = quote_mcp_metadata_label(
        raw_tool_name,
        MAX_MCP_DESCRIPTION_RAW_TOOL_LABEL_BYTES,
        "tool",
    );
    let header = format!("MCP server: {server_label}\nMCP tool: {tool_label}\nDescription: ");
    let description = normalize_mcp_metadata_body(description.unwrap_or_default());
    let body = if description.is_empty() {
        "No description was provided by the server.".to_string()
    } else {
        description
    };
    let available_body_bytes = MAX_MCP_DESCRIPTION_BYTES.saturating_sub(header.len());
    let truncated = body.len() > available_body_bytes;
    let body_budget = if truncated {
        available_body_bytes.saturating_sub(MCP_DESCRIPTION_TRUNCATION_MARKER.len())
    } else {
        available_body_bytes
    };
    let mut normalized = String::with_capacity(MAX_MCP_DESCRIPTION_BYTES);
    normalized.push_str(&header);
    normalized.push_str(&truncate_utf8(&body, body_budget));
    if truncated {
        normalized.push_str(MCP_DESCRIPTION_TRUNCATION_MARKER);
    }
    truncate_utf8(&normalized, MAX_MCP_DESCRIPTION_BYTES)
}

fn quote_mcp_metadata_label(value: &str, max_bytes: usize, fallback: &str) -> String {
    let raw_value = value;
    let (mut value, truncated) = normalize_mcp_metadata_label_with_status(raw_value, max_bytes);
    if truncated {
        let digest = format!("{:x}", Sha256::digest(raw_value.as_bytes()));
        let marker = format!("… [truncated #{}]", &digest[..8]);
        let prefix_budget = max_bytes.saturating_sub(marker.len());
        value = format!("{}{}", truncate_utf8(&value, prefix_budget), marker);
    }
    serde_json::to_string(if value.is_empty() { fallback } else { &value })
        .unwrap_or_else(|_| format!("\"{fallback}\""))
}

fn normalize_mcp_metadata_label(value: &str, max_bytes: usize) -> String {
    normalize_mcp_metadata_label_with_status(value, max_bytes).0
}

fn normalize_mcp_metadata_label_with_status(value: &str, max_bytes: usize) -> (String, bool) {
    let mut normalized = String::new();
    let mut pending_space = false;
    let mut truncated = false;
    for character in value.chars() {
        if character.is_whitespace() || is_unsafe_mcp_metadata_character(character) {
            pending_space = !normalized.is_empty();
            continue;
        }
        if pending_space {
            if normalized.len() + 1 > max_bytes {
                truncated = true;
                break;
            }
            normalized.push(' ');
            pending_space = false;
        }
        if normalized.len() + character.len_utf8() > max_bytes {
            truncated = true;
            break;
        }
        normalized.push(character);
    }
    (normalized.trim().to_string(), truncated)
}

fn normalize_mcp_metadata_body(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len().min(MAX_MCP_DESCRIPTION_BYTES));
    let mut previous_was_cr = false;
    for character in value.chars() {
        if character == '\r' {
            normalized.push('\n');
            previous_was_cr = true;
        } else if character == '\n' {
            if !previous_was_cr {
                normalized.push('\n');
            }
            previous_was_cr = false;
        } else {
            previous_was_cr = false;
            normalized.push(
                if character == '\t' || is_unsafe_mcp_metadata_character(character) {
                    ' '
                } else {
                    character
                },
            );
        }
        if normalized.len() > MAX_MCP_DESCRIPTION_BYTES {
            break;
        }
    }
    normalized.trim().to_string()
}

fn is_unsafe_mcp_metadata_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{00ad}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061c}'
                | '\u{06dd}'
                | '\u{070f}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08e2}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{110bd}'
                | '\u{110cd}'
                | '\u{13430}'..='\u{1345f}'
                | '\u{1bca0}'..='\u{1bca3}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0001}'
                | '\u{e0020}'..='\u{e007f}'
        )
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
