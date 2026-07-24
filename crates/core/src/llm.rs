mod payload;
mod response;
mod stream;
mod tool_call_id;
mod transport;

pub(crate) use tool_call_id::{
    model_response_tool_call_id, validate_model_tool_call_id, validate_model_tool_protocol,
};
pub(crate) use transport::{
    complete_chat, complete_chat_allow_empty, complete_chat_streaming,
    complete_chat_streaming_allow_empty, is_repairable_empty_model_action,
};

use crate::cancellation::AgentCancellationToken;
use crate::protocol::{AgentApiStyle, AgentError, AgentResult, AgentToolDefinition, AgentUsage};
use crate::tools::schema::validate_portable_tool_input_schema;
use crate::usage::{extract_usage, merge_total_usage, usage_for_request};
use payload::{build_headers, build_payload, is_sse_response};
use response::{
    extract_api_error, extract_finish_reason, extract_response_text, extract_tool_calls,
    truncate_for_error,
};
use serde_json::{json, Value};
use std::time::Duration;
use stream::parse_sse_response;
#[cfg(test)]
use stream::{process_sse_frame, LlmStreamAccumulator};

#[derive(Debug, Clone)]
pub(crate) struct LlmChatRequest {
    pub api_url: String,
    pub api_token: String,
    pub model: String,
    pub api_style: AgentApiStyle,
    pub max_tokens: u32,
    pub temperature: f32,
    pub stream: bool,
    pub messages: Vec<LlmMessage>,
    pub tools: Vec<AgentToolDefinition>,
}

#[derive(Debug, Clone)]
pub(crate) struct LlmChatResponse {
    pub content: String,
    pub tool_calls: Vec<LlmToolCall>,
    pub usage: Option<AgentUsage>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LlmMessage {
    pub role: LlmMessageRole,
    pub content: String,
    pub images: Vec<LlmImage>,
    pub tool_call_id: Option<String>,
    pub tool_calls: Vec<LlmToolCall>,
    pub is_error: bool,
}

impl LlmMessage {
    pub(crate) fn text(role: LlmMessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            images: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            is_error: false,
        }
    }

    pub(crate) fn assistant(content: impl Into<String>, tool_calls: Vec<LlmToolCall>) -> Self {
        Self {
            role: LlmMessageRole::Assistant,
            content: content.into(),
            images: Vec::new(),
            tool_call_id: None,
            tool_calls,
            is_error: false,
        }
    }

    pub(crate) fn tool_result(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: LlmMessageRole::Tool,
            content: content.into(),
            images: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: Vec::new(),
            is_error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LlmMessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LlmImage {
    pub mime_type: String,
    pub data_base64: String,
}

impl LlmMessageRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone)]
pub(crate) enum LlmStreamEvent {
    AttemptStarted {
        attempt: usize,
        max_attempts: usize,
    },
    Delta(String),
    ToolInputProgress {
        tool_call_index: usize,
        tool: String,
        input_delta: String,
        received_bytes: u64,
    },
    AttemptReset {
        reason: String,
    },
    Retrying {
        attempt: usize,
        max_attempts: usize,
        reason: String,
    },
    Committed,
}

pub(crate) fn detect_api_style(api_url: &str) -> AgentApiStyle {
    let normalized = api_url.to_ascii_lowercase();

    if normalized.contains("/chat/completions") {
        return AgentApiStyle::OpenAiCompatible;
    }

    if normalized.contains("anthropic") || normalized.ends_with("/messages") {
        return AgentApiStyle::AnthropicCompatible;
    }

    AgentApiStyle::OpenAiCompatible
}

#[cfg(test)]
mod tests;
