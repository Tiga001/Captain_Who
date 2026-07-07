// LLM request payload and HTTP header builders.
use super::{LlmChatRequest, LlmMessage, LlmMessageRole, LlmToolCall};
use crate::protocol::{AgentApiStyle, AgentError, AgentResult, AgentToolDefinition};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Map, Value};

pub(super) fn build_payload(request: &LlmChatRequest) -> Value {
    match request.api_style {
        AgentApiStyle::OpenAiCompatible => {
            let mut payload = Map::from_iter([
                ("model".to_string(), json!(request.model)),
                (
                    "messages".to_string(),
                    Value::Array(build_openai_messages(&request.messages)),
                ),
                ("stream".to_string(), json!(request.stream)),
                ("max_tokens".to_string(), json!(request.max_tokens)),
            ]);
            if should_send_temperature(&request.model) {
                payload.insert("temperature".to_string(), json!(request.temperature));
            }
            if request.stream {
                payload.insert(
                    "stream_options".to_string(),
                    json!({ "include_usage": true }),
                );
            }

            if !request.tools.is_empty() {
                payload.insert(
                    "tools".to_string(),
                    Value::Array(build_openai_tools(&request.tools)),
                );
                payload.insert("tool_choice".to_string(), json!("auto"));
            }

            Value::Object(payload)
        }
        AgentApiStyle::AnthropicCompatible => {
            let (system, messages) = split_anthropic_messages(&request.messages);
            let mut payload = Map::from_iter([
                ("model".to_string(), json!(request.model)),
                ("max_tokens".to_string(), json!(request.max_tokens)),
                ("messages".to_string(), json!(messages)),
            ]);
            if should_send_temperature(&request.model) {
                payload.insert("temperature".to_string(), json!(request.temperature));
            }

            if request.stream {
                payload.insert("stream".to_string(), json!(true));
            }
            if let Some(system) = system {
                payload.insert("system".to_string(), json!(system));
            }
            if !request.tools.is_empty() {
                payload.insert(
                    "tools".to_string(),
                    Value::Array(build_anthropic_tools(&request.tools)),
                );
            }

            Value::Object(payload)
        }
    }
}

fn should_send_temperature(model: &str) -> bool {
    !model.to_ascii_lowercase().contains("claude")
}

pub(super) fn is_sse_response(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_ascii_lowercase().contains("text/event-stream"))
        .unwrap_or(false)
}

fn build_openai_messages(messages: &[LlmMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| match message.role {
            LlmMessageRole::System => json!({ "role": "system", "content": message.content }),
            LlmMessageRole::User => json!({
                "role": "user",
                "content": build_openai_user_content(message)
            }),
            LlmMessageRole::Assistant => {
                let mut object = Map::from_iter([(
                    "role".to_string(),
                    Value::String(message.role.as_str().to_string()),
                )]);
                if message.tool_calls.is_empty() {
                    object.insert("content".to_string(), json!(message.content));
                } else {
                    object.insert(
                        "content".to_string(),
                        if message.content.trim().is_empty() {
                            Value::Null
                        } else {
                            json!(message.content)
                        },
                    );
                    object.insert(
                        "tool_calls".to_string(),
                        Value::Array(build_openai_tool_calls(&message.tool_calls)),
                    );
                }
                Value::Object(object)
            }
            LlmMessageRole::Tool => json!({
                "role": "tool",
                "tool_call_id": message.tool_call_id.as_deref().unwrap_or_default(),
                "content": message.content
            }),
        })
        .collect()
}

fn build_openai_user_content(message: &LlmMessage) -> Value {
    if message.images.is_empty() {
        return json!(message.content);
    }

    let mut parts = Vec::new();
    if !message.content.trim().is_empty() {
        parts.push(json!({
            "type": "text",
            "text": message.content
        }));
    }
    parts.extend(message.images.iter().map(|image| {
        json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:{};base64,{}", image.mime_type, image.data_base64)
            }
        })
    }));

    Value::Array(parts)
}

fn build_openai_tools(tools: &[AgentToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": normalize_json_schema(&tool.input_schema)
                }
            })
        })
        .collect()
}

fn build_openai_tool_calls(tool_calls: &[LlmToolCall]) -> Vec<Value> {
    tool_calls
        .iter()
        .map(|call| {
            json!({
                "id": call.id,
                "type": "function",
                "function": {
                    "name": call.name,
                    "arguments": serde_json::to_string(&call.args).unwrap_or_else(|_| "{}".to_string())
                }
            })
        })
        .collect()
}

fn split_anthropic_messages(messages: &[LlmMessage]) -> (Option<String>, Vec<Value>) {
    let mut system_parts = Vec::new();
    let mut chat_messages = Vec::new();

    for message in messages {
        match message.role {
            LlmMessageRole::System => system_parts.push(message.content.as_str()),
            LlmMessageRole::User => {
                let mut blocks = Vec::new();
                if !message.content.trim().is_empty() {
                    blocks.push(json!({ "type": "text", "text": message.content }));
                }
                blocks.extend(message.images.iter().map(|image| {
                    json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": image.mime_type,
                            "data": image.data_base64
                        }
                    })
                }));
                push_anthropic_message(&mut chat_messages, "user", blocks);
            }
            LlmMessageRole::Assistant => {
                let mut blocks = Vec::new();
                if !message.content.trim().is_empty() {
                    blocks.push(json!({ "type": "text", "text": message.content }));
                }
                blocks.extend(message.tool_calls.iter().map(|call| {
                    json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.args
                    })
                }));
                push_anthropic_message(&mut chat_messages, "assistant", blocks);
            }
            LlmMessageRole::Tool => {
                push_anthropic_message(
                    &mut chat_messages,
                    "user",
                    vec![json!({
                        "type": "tool_result",
                        "tool_use_id": message.tool_call_id.as_deref().unwrap_or_default(),
                        "content": message.content,
                        "is_error": message.is_error
                    })],
                );
            }
        }
    }

    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };

    (system, chat_messages)
}

fn push_anthropic_message(messages: &mut Vec<Value>, role: &str, content_blocks: Vec<Value>) {
    if content_blocks.is_empty() {
        return;
    }

    if let Some(last) = messages.last_mut() {
        let same_role = last
            .get("role")
            .and_then(Value::as_str)
            .map(|value| value == role)
            .unwrap_or(false);
        if same_role {
            if let Some(content) = last.get_mut("content").and_then(Value::as_array_mut) {
                content.extend(content_blocks);
                return;
            }
        }
    }

    messages.push(json!({
        "role": role,
        "content": content_blocks
    }));
}

fn build_anthropic_tools(tools: &[AgentToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": normalize_json_schema(&tool.input_schema)
            })
        })
        .collect()
}

fn normalize_json_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(_) => schema.clone(),
        _ => json!({ "type": "object", "properties": {} }),
    }
}

pub(super) fn build_headers(api_style: AgentApiStyle, api_token: &str) -> AgentResult<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    match api_style {
        AgentApiStyle::OpenAiCompatible => {
            let bearer = format!("Bearer {api_token}");
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&bearer)
                    .map_err(|_| AgentError::new("API Token 包含非法字符。"))?,
            );
        }
        AgentApiStyle::AnthropicCompatible => {
            headers.insert(
                "x-api-key",
                HeaderValue::from_str(api_token)
                    .map_err(|_| AgentError::new("API Token 包含非法字符。"))?,
            );
            headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        }
    }

    Ok(headers)
}
