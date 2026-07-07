// LLM response extraction and error formatting helpers.
use super::LlmToolCall;
use crate::protocol::{AgentApiStyle, AgentError, AgentResult};
use serde_json::{json, Value};

pub(super) fn extract_api_error(value: &Value) -> Option<String> {
    let error = value.get("error")?;
    if let Some(message) = error
        .as_str()
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        return Some(message.to_string());
    }

    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.get("error").and_then(Value::as_str))
        .or_else(|| error.get("type").and_then(Value::as_str))
        .map(str::trim)
        .filter(|message| !message.is_empty());
    if let Some(message) = message {
        return Some(message.to_string());
    }

    Some(truncate_for_error(&error.to_string()))
}

pub(super) fn extract_response_text(value: &Value) -> Option<String> {
    if let Some(text) = value.get("content").and_then(extract_content_text) {
        return Some(text);
    }

    if let Some(text) = value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(extract_content_text)
    {
        return Some(text);
    }

    if let Some(text) = value.get("completion").and_then(Value::as_str) {
        return Some(text.to_string());
    }

    let choice = value.get("choices")?.as_array()?.first()?;

    if let Some(text) = choice
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(extract_content_text)
    {
        return Some(text);
    }

    choice
        .get("text")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn extract_content_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }

    let parts = content.as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .or_else(|| part.get("content").and_then(Value::as_str))
        })
        .collect::<Vec<_>>()
        .join("");

    Some(text)
}

pub(super) fn extract_finish_reason(value: &Value) -> Option<String> {
    if let Some(reason) = value.get("stop_reason").and_then(Value::as_str) {
        return Some(reason.to_string());
    }

    let choice = value.get("choices")?.as_array()?.first()?;
    choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

pub(super) fn extract_tool_calls(
    value: &Value,
    api_style: AgentApiStyle,
) -> AgentResult<Vec<LlmToolCall>> {
    match api_style {
        AgentApiStyle::OpenAiCompatible => extract_openai_tool_calls(value),
        AgentApiStyle::AnthropicCompatible => extract_anthropic_tool_calls(value),
    }
}

fn extract_openai_tool_calls(value: &Value) -> AgentResult<Vec<LlmToolCall>> {
    let Some(calls) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("tool_calls"))
        .and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };

    calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            let function = call
                .get("function")
                .ok_or_else(|| AgentError::new("OpenAI tool_call 缺少 function 字段。"))?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| AgentError::new("OpenAI tool_call 缺少 function.name。"))?;
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let args = parse_tool_arguments(arguments).map_err(|error| {
                AgentError::new(format!(
                    "OpenAI tool_call `{name}` 的 arguments 不是有效 JSON：{error}"
                ))
            })?;
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("openai-tool-call-{}", index + 1));

            Ok(LlmToolCall {
                id,
                name: name.to_string(),
                args,
            })
        })
        .collect()
}

fn extract_anthropic_tool_calls(value: &Value) -> AgentResult<Vec<LlmToolCall>> {
    let Some(content) = value.get("content").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    content
        .iter()
        .filter(|part| {
            part.get("type")
                .and_then(Value::as_str)
                .map(|kind| kind == "tool_use")
                .unwrap_or(false)
        })
        .enumerate()
        .map(|(index, part)| {
            let name = part
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| AgentError::new("Anthropic tool_use 缺少 name。"))?;
            let id = part
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("anthropic-tool-use-{}", index + 1));
            let args = part.get("input").cloned().unwrap_or_else(|| json!({}));

            Ok(LlmToolCall {
                id,
                name: name.to_string(),
                args,
            })
        })
        .collect()
}

pub(super) fn parse_tool_arguments(arguments: &str) -> serde_json::Result<Value> {
    let arguments = arguments.trim();
    if arguments.is_empty() {
        return Ok(json!({}));
    }

    serde_json::from_str(arguments)
}

pub(super) fn truncate_for_error(value: &str) -> String {
    const MAX_CHARS: usize = 600;

    let mut truncated = value.chars().take(MAX_CHARS).collect::<String>();
    if value.chars().count() > MAX_CHARS {
        truncated.push('…');
    }

    truncated
}
