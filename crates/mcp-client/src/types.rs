use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::McpError;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct McpServerId(Uuid);

impl McpServerId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for McpServerId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for McpServerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("McpServerId").field(&self.0).finish()
    }
}

impl fmt::Display for McpServerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for McpServerId {
    type Err = McpError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value)
            .map(Self)
            .map_err(|error| McpError::config(format!("invalid MCP server ID: {error}")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpLifecycleKind {
    Discover,
    InitializeFallback,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpImplementationInfo {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCapabilitySnapshot {
    pub tools: bool,
    pub tools_list_changed: bool,
    pub resources: bool,
    pub resources_list_changed: bool,
    pub resources_subscribe: bool,
    pub prompts: bool,
    pub prompts_list_changed: bool,
    pub logging: bool,
    pub completions: bool,
    pub tasks: bool,
    #[serde(default)]
    pub extensions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpProtocolSnapshot {
    pub negotiated_version: String,
    pub lifecycle: McpLifecycleKind,
    pub server: Option<McpImplementationInfo>,
    pub capabilities: McpCapabilitySnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpConnectionState {
    Connecting,
    Ready,
    Closing,
    Closed,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolAnnotations {
    pub title: Option<String>,
    pub read_only_hint: Option<bool>,
    pub destructive_hint: Option<bool>,
    pub idempotent_hint: Option<bool>,
    pub open_world_hint: Option<bool>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDescriptor {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
    pub annotations: Option<McpToolAnnotations>,
}

impl fmt::Debug for McpToolDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpToolDescriptor")
            .field("name", &self.name)
            .field("has_title", &self.title.is_some())
            .field(
                "description_bytes",
                &self.description.as_ref().map(String::len),
            )
            .field("input_schema", &"<redacted>")
            .field("has_output_schema", &self.output_schema.is_some())
            .field("annotations", &self.annotations)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpCacheScope {
    Private,
    Public,
    Unknown,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolPage {
    pub tools: Vec<McpToolDescriptor>,
    pub next_cursor: Option<String>,
    pub ttl_ms: Option<u64>,
    pub cache_scope: Option<McpCacheScope>,
}

impl fmt::Debug for McpToolPage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpToolPage")
            .field("tool_count", &self.tools.len())
            .field("has_next_cursor", &self.next_cursor.is_some())
            .field("ttl_ms", &self.ttl_ms)
            .field("cache_scope", &self.cache_scope)
            .finish()
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCall {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
    pub timeout_ms: Option<u64>,
}

impl fmt::Debug for McpToolCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpToolCall")
            .field("name", &self.name)
            .field("arguments", &"<redacted>")
            .field("timeout_ms", &self.timeout_ms)
            .finish()
    }
}

impl McpToolCall {
    pub fn new(name: impl Into<String>, arguments: Value) -> Self {
        Self {
            name: name.into(),
            arguments,
            timeout_ms: None,
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpContentBlock {
    Text { text: String },
    Image { data: String, mime_type: String },
    Audio { data: String, mime_type: String },
    EmbeddedResource { resource: McpEmbeddedResource },
    ResourceLink { resource: McpResourceLink },
}

impl fmt::Debug for McpContentBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { text } => formatter
                .debug_struct("Text")
                .field("bytes", &text.len())
                .finish(),
            Self::Image { data, mime_type } => formatter
                .debug_struct("Image")
                .field("encoded_bytes", &data.len())
                .field("mime_type", mime_type)
                .finish(),
            Self::Audio { data, mime_type } => formatter
                .debug_struct("Audio")
                .field("encoded_bytes", &data.len())
                .field("mime_type", mime_type)
                .finish(),
            Self::EmbeddedResource { resource } => formatter
                .debug_struct("EmbeddedResource")
                .field("resource", resource)
                .finish(),
            Self::ResourceLink { resource } => formatter
                .debug_struct("ResourceLink")
                .field("resource", resource)
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "contentType", rename_all = "snake_case")]
pub enum McpEmbeddedResource {
    Text {
        uri: String,
        mime_type: Option<String>,
        text: String,
    },
    Blob {
        uri: String,
        mime_type: Option<String>,
        data: String,
    },
}

impl fmt::Debug for McpEmbeddedResource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text {
                mime_type, text, ..
            } => formatter
                .debug_struct("Text")
                .field("uri", &"<redacted>")
                .field("mime_type", mime_type)
                .field("bytes", &text.len())
                .finish(),
            Self::Blob {
                mime_type, data, ..
            } => formatter
                .debug_struct("Blob")
                .field("uri", &"<redacted>")
                .field("mime_type", mime_type)
                .field("encoded_bytes", &data.len())
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResourceLink {
    pub uri: String,
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub mime_type: Option<String>,
    pub size: Option<u64>,
}

impl fmt::Debug for McpResourceLink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpResourceLink")
            .field("uri", &"<redacted>")
            .field("name", &self.name)
            .field("has_title", &self.title.is_some())
            .field(
                "description_bytes",
                &self.description.as_ref().map(String::len),
            )
            .field("mime_type", &self.mime_type)
            .field("size", &self.size)
            .finish()
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolResult {
    pub content: Vec<McpContentBlock>,
    pub structured_content: Option<Value>,
    pub is_error: bool,
}

impl fmt::Debug for McpToolResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpToolResult")
            .field("content_blocks", &self.content.len())
            .field("has_structured_content", &self.structured_content.is_some())
            .field("is_error", &self.is_error)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_domain_debug_omits_payloads_and_content() {
        let call = McpToolCall::new("echo_text", json!({"token": "sensitive-call-value"}));
        assert!(!format!("{call:?}").contains("sensitive-call-value"));

        let result = McpToolResult {
            content: vec![McpContentBlock::Text {
                text: "sensitive-result-value".to_string(),
            }],
            structured_content: Some(json!({"token": "sensitive-structured-value"})),
            is_error: false,
        };
        let rendered = format!("{result:?}");
        assert!(!rendered.contains("sensitive-result-value"));
        assert!(!rendered.contains("sensitive-structured-value"));
    }
}
