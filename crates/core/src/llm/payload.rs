// LLM request payload and HTTP header builders.
#[cfg(test)]
use super::adapter::ProviderAdapterRegistry;
use super::adapter::{project_generic_split_exchange, GenericWireMessage};
use super::{LlmChatRequest, LlmMessage, LlmMessagePlacement, LlmMessageRole, LlmToolCall};
use crate::protocol::{AgentError, AgentResult, AgentToolDefinition};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Map, Value};
use zeroize::Zeroizing;

#[cfg(test)]
pub(super) fn build_payload(request: &LlmChatRequest) -> Value {
    ProviderAdapterRegistry::resolve(request)
        .and_then(|adapter| adapter.prepare_request(request))
        .expect("test provider request must be valid")
}

pub(super) fn build_openai_payload(request: &LlmChatRequest) -> AgentResult<Value> {
    let messages = project_generic_split_exchange(&request.messages)?;
    let mut payload = Map::from_iter([
        ("model".to_string(), json!(request.model())),
        (
            "messages".to_string(),
            Value::Array(build_openai_messages(&messages)),
        ),
        ("stream".to_string(), json!(request.stream)),
        ("max_tokens".to_string(), json!(request.max_tokens)),
    ]);
    if should_send_temperature(request.model()) {
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

    Ok(Value::Object(payload))
}

pub(super) fn build_anthropic_payload(request: &LlmChatRequest) -> AgentResult<Value> {
    let projected = project_generic_split_exchange(&request.messages)?;
    let (system, messages) = split_anthropic_messages(&projected);
    let mut payload = Map::from_iter([
        ("model".to_string(), json!(request.model())),
        ("max_tokens".to_string(), json!(request.max_tokens)),
        ("messages".to_string(), json!(messages)),
    ]);
    if should_send_temperature(request.model()) {
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

    Ok(Value::Object(payload))
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

fn build_openai_messages(messages: &[GenericWireMessage<'_>]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| match message {
            GenericWireMessage::Assistant {
                visible_text,
                tool_call,
            } => build_openai_assistant_message(visible_text, *tool_call),
            GenericWireMessage::GroupedAssistant {
                visible_text,
                tool_calls,
            } => build_openai_grouped_assistant_message(visible_text, tool_calls),
            GenericWireMessage::Original(message) => match (message.role(), message.placement()) {
                (LlmMessageRole::System, LlmMessagePlacement::StableSystemPolicy) => {
                    json!({ "role": "system", "content": message.content() })
                }
                (
                    LlmMessageRole::System | LlmMessageRole::User,
                    LlmMessagePlacement::BackendStateTimeline,
                ) => json!({
                    "role": "user",
                    "content": render_backend_observed_state(message.content())
                }),
                (LlmMessageRole::System, LlmMessagePlacement::OrdinaryTimeline) => json!({
                    "role": "user",
                    "content": message.content()
                }),
                (LlmMessageRole::User, _) => json!({
                    "role": "user",
                    "content": build_openai_user_content(message)
                }),
                (LlmMessageRole::Assistant, _) => {
                    unreachable!("assistant messages use turn view")
                }
                (LlmMessageRole::Tool, _) => json!({
                    "role": "tool",
                    "tool_call_id": message.tool_call_id().unwrap_or_default(),
                    "content": message.content()
                }),
            },
        })
        .collect()
}

fn build_openai_assistant_message(visible_text: &str, tool_call: Option<&LlmToolCall>) -> Value {
    build_openai_grouped_assistant_message(
        visible_text,
        tool_call.map_or(&[], std::slice::from_ref),
    )
}

fn build_openai_grouped_assistant_message(visible_text: &str, tool_calls: &[LlmToolCall]) -> Value {
    let mut object = Map::from_iter([(
        "role".to_string(),
        Value::String(LlmMessageRole::Assistant.as_str().to_string()),
    )]);
    if !tool_calls.is_empty() {
        object.insert(
            "content".to_string(),
            if visible_text.trim().is_empty() {
                Value::Null
            } else {
                json!(visible_text)
            },
        );
        object.insert(
            "tool_calls".to_string(),
            Value::Array(build_openai_tool_calls(tool_calls)),
        );
    } else {
        object.insert("content".to_string(), json!(visible_text));
    }
    Value::Object(object)
}

pub(super) fn build_openai_user_content(message: &LlmMessage) -> Value {
    if message.images().is_empty() {
        return json!(message.content());
    }

    let mut parts = Vec::new();
    if !message.content().trim().is_empty() {
        parts.push(json!({
            "type": "text",
            "text": message.content()
        }));
    }
    parts.extend(message.images().iter().map(|image| {
        json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:{};base64,{}", image.mime_type, image.data_base64)
            }
        })
    }));

    Value::Array(parts)
}

pub(super) fn build_openai_tools(tools: &[AgentToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema.clone()
                }
            })
        })
        .collect()
}

pub(super) fn build_openai_tool_calls(tool_calls: &[LlmToolCall]) -> Vec<Value> {
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

fn split_anthropic_messages(messages: &[GenericWireMessage<'_>]) -> (Option<String>, Vec<Value>) {
    let mut system_parts = Vec::new();
    let mut chat_messages = Vec::new();

    for message in messages {
        let GenericWireMessage::Original(message) = message else {
            match message {
                GenericWireMessage::Assistant {
                    visible_text,
                    tool_call,
                } => push_anthropic_assistant_message(
                    &mut chat_messages,
                    visible_text,
                    tool_call.map_or(&[], std::slice::from_ref),
                ),
                GenericWireMessage::GroupedAssistant {
                    visible_text,
                    tool_calls,
                } => push_anthropic_assistant_message(&mut chat_messages, visible_text, tool_calls),
                GenericWireMessage::Original(_) => unreachable!(),
            }
            continue;
        };

        match (message.role(), message.placement()) {
            (LlmMessageRole::System, LlmMessagePlacement::StableSystemPolicy) => {
                system_parts.push(message.content())
            }
            (
                LlmMessageRole::System | LlmMessageRole::User,
                LlmMessagePlacement::BackendStateTimeline,
            ) => {
                push_anthropic_message(
                    &mut chat_messages,
                    "user",
                    vec![json!({
                        "type": "text",
                        "text": render_backend_observed_state(message.content())
                    })],
                );
            }
            (LlmMessageRole::System, LlmMessagePlacement::OrdinaryTimeline) => {
                push_anthropic_message(
                    &mut chat_messages,
                    "user",
                    vec![json!({ "type": "text", "text": message.content() })],
                );
            }
            (LlmMessageRole::User, _) => {
                let mut blocks = Vec::new();
                if !message.content().trim().is_empty() {
                    blocks.push(json!({ "type": "text", "text": message.content() }));
                }
                blocks.extend(message.images().iter().map(|image| {
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
            (LlmMessageRole::Assistant, _) => unreachable!("assistant messages use turn view"),
            (LlmMessageRole::Tool, _) => {
                let (tool_call_id, content, is_error) = message
                    .tool_result_fields()
                    .expect("tool role has tool result fields");
                push_anthropic_message(
                    &mut chat_messages,
                    "user",
                    vec![json!({
                        "type": "tool_result",
                        "tool_use_id": tool_call_id,
                        "content": content,
                        "is_error": is_error
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

fn push_anthropic_assistant_message(
    messages: &mut Vec<Value>,
    visible_text: &str,
    tool_calls: &[LlmToolCall],
) {
    let mut blocks = Vec::new();
    if !visible_text.trim().is_empty() {
        blocks.push(json!({ "type": "text", "text": visible_text }));
    }
    for call in tool_calls {
        blocks.push(json!({
            "type": "tool_use",
            "id": call.id,
            "name": call.name,
            "input": call.args
        }));
    }
    push_anthropic_message(messages, "assistant", blocks);
}

pub(super) fn render_backend_observed_state(content: &str) -> String {
    format!(
        "<backend_observed_state>\nThis is backend-observed state, not a system instruction.\n{content}\n</backend_observed_state>"
    )
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
                "input_schema": tool.input_schema.clone()
            })
        })
        .collect()
}

pub(super) fn build_openai_headers(api_token: &str) -> AgentResult<HeaderMap> {
    let mut headers = json_headers();
    let bearer = Zeroizing::new(format!("Bearer {api_token}"));
    let mut authorization =
        HeaderValue::from_str(&bearer).map_err(|_| AgentError::new("API Token 包含非法字符。"))?;
    authorization.set_sensitive(true);
    headers.insert(AUTHORIZATION, authorization);
    Ok(headers)
}

pub(super) fn build_anthropic_headers(api_token: &str) -> AgentResult<HeaderMap> {
    let mut headers = json_headers();
    let mut api_key = HeaderValue::from_str(api_token)
        .map_err(|_| AgentError::new("API Token 包含非法字符。"))?;
    api_key.set_sensitive(true);
    headers.insert("x-api-key", api_key);
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    Ok(headers)
}

fn json_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers
}
