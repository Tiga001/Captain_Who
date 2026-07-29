use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{McpError, McpServerConfig, McpTransportConfig};

const CONFIG_DIGEST_DOMAIN: &[u8] = b"mycopilot-mcp-config-v1\0";
const SCHEMA_DIGEST_DOMAIN: &[u8] = b"mycopilot-mcp-schema-v1\0";
const CATALOG_DIGEST_DOMAIN: &[u8] = b"mycopilot-mcp-catalog-v1\0";
const TOOL_NAME_DIGEST_DOMAIN: &[u8] = b"mycopilot-mcp-tool-name-v1\0";
const MAX_CANONICAL_JSON_DEPTH: usize = 128;

macro_rules! digest_type {
    ($name:ident) => {
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.0)
                    .finish()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

digest_type!(McpConfigDigest);
digest_type!(McpSchemaDigest);
digest_type!(McpCatalogDigest);

impl FromStr for McpConfigDigest {
    type Err = McpError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(McpError::config("invalid MCP configuration digest"));
        }
        Ok(Self(value.to_string()))
    }
}

impl FromStr for McpCatalogDigest {
    type Err = McpError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(McpError::config("invalid MCP catalog digest"));
        }
        Ok(Self(value.to_string()))
    }
}

pub fn config_digest(config: &McpServerConfig) -> Result<McpConfigDigest, McpError> {
    let mut normalized = config.clone();
    match &mut normalized.transport {
        McpTransportConfig::Stdio(stdio) => {
            stdio.environment.sort_by(|left, right| {
                left.name()
                    .cmp(right.name())
                    .then_with(|| binding_rank(left).cmp(&binding_rank(right)))
            });
        }
    }
    let value = serde_json::to_value(normalized)
        .map_err(|_| McpError::config("MCP configuration could not be normalized"))?;
    Ok(McpConfigDigest(hash_canonical_json(
        CONFIG_DIGEST_DOMAIN,
        &value,
    )?))
}

pub(crate) fn schema_digest(
    input_schema: &Value,
    output_schema: Option<&Value>,
) -> Result<McpSchemaDigest, McpError> {
    let value = Value::Array(vec![
        input_schema.clone(),
        output_schema.cloned().unwrap_or(Value::Null),
    ]);
    Ok(McpSchemaDigest(hash_canonical_json(
        SCHEMA_DIGEST_DOMAIN,
        &value,
    )?))
}

pub(crate) fn catalog_digest(value: &Value) -> Result<McpCatalogDigest, McpError> {
    Ok(McpCatalogDigest(hash_canonical_json(
        CATALOG_DIGEST_DOMAIN,
        value,
    )?))
}

pub(crate) fn tool_name_hash(raw_name: &str) -> String {
    hash_bytes(TOOL_NAME_DIGEST_DOMAIN, raw_name.as_bytes())[..12].to_string()
}

pub(crate) fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>, McpError> {
    let canonical = canonicalize(value, 0)?;
    serde_json::to_vec(&canonical)
        .map_err(|_| McpError::protocol("MCP JSON value could not be normalized"))
}

fn hash_canonical_json(domain: &[u8], value: &Value) -> Result<String, McpError> {
    Ok(hash_bytes(domain, &canonical_json_bytes(value)?))
}

fn hash_bytes(domain: &[u8], bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn canonicalize(value: &Value, depth: usize) -> Result<Value, McpError> {
    if depth > MAX_CANONICAL_JSON_DEPTH {
        return Err(McpError::protocol(
            "MCP JSON value exceeded the normalization depth limit",
        ));
    }
    match value {
        Value::Array(values) => values
            .iter()
            .map(|value| canonicalize(value, depth + 1))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            let mut normalized = Map::new();
            for key in keys {
                normalized.insert(key.clone(), canonicalize(&values[key], depth + 1)?);
            }
            Ok(Value::Object(normalized))
        }
        _ => Ok(value.clone()),
    }
}

fn binding_rank(binding: &crate::McpEnvBinding) -> u8 {
    match binding {
        crate::McpEnvBinding::Plain { .. } => 0,
        crate::McpEnvBinding::SecretRef { .. } => 1,
        crate::McpEnvBinding::AllowlistedHostVariable { .. } => 2,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::{McpEnvBinding, McpServerId, McpServerScope, McpStdioConfig, McpTrustLevel};

    fn config(environment: Vec<McpEnvBinding>) -> McpServerConfig {
        McpServerConfig {
            id: McpServerId::new(),
            display_name: "digest fixture".to_string(),
            scope: McpServerScope::User,
            trust: McpTrustLevel::UserApproved,
            enabled: true,
            transport: McpTransportConfig::Stdio(McpStdioConfig {
                program: PathBuf::from("/owned/fixture"),
                arguments: vec!["first".to_string(), "second".to_string()],
                cwd: PathBuf::from("/owned"),
                environment,
            }),
            connect_timeout_ms: 1_000,
            request_timeout_ms: 2_000,
            shutdown_timeout_ms: 3_000,
        }
    }

    #[test]
    fn environment_order_does_not_change_config_digest() {
        let first = McpEnvBinding::Plain {
            name: "FIRST".to_string(),
            value: "one".to_string(),
        };
        let second = McpEnvBinding::SecretRef {
            name: "SECOND".to_string(),
            secret_id: "opaque-reference".to_string(),
        };
        let left = config(vec![first.clone(), second.clone()]);
        let mut right = config(vec![second, first]);
        right.id = left.id;
        assert_eq!(
            config_digest(&left).unwrap(),
            config_digest(&right).unwrap()
        );
    }

    #[test]
    fn canonical_schema_digest_ignores_object_key_order() {
        let left = json!({"type": "object", "properties": {"b": {"type":"string"}, "a": {"type":"number"}}});
        let right = json!({"properties": {"a": {"type":"number"}, "b": {"type":"string"}}, "type": "object"});
        assert_eq!(
            schema_digest(&left, None).unwrap(),
            schema_digest(&right, None).unwrap()
        );
    }

    #[test]
    fn configuration_digest_parser_is_strict_and_safe() {
        let digest = "0123456789abcdef".repeat(4);
        assert_eq!(digest.parse::<McpConfigDigest>().unwrap().as_str(), digest);
        assert_eq!(digest.parse::<McpCatalogDigest>().unwrap().as_str(), digest);
        assert!("A".repeat(64).parse::<McpConfigDigest>().is_err());
        assert!("0".repeat(63).parse::<McpConfigDigest>().is_err());
        assert!("g".repeat(64).parse::<McpCatalogDigest>().is_err());
    }
}
