//! A disposable, read-only sample beside a completed conversation. Tool definitions stay
//! in the prefix for provider replay, but no runtime, tool registry or event emitter runs here.

use super::*;
use crate::llm::{complete_prompt_prediction_once, LlmAssistantTurn};
use crate::AgentUsage;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::mem::{size_of, size_of_val};
use std::time::Duration;
use zeroize::Zeroize;

const OUTPUT_TOKENS: u32 = 512;
const DEADLINE: Duration = Duration::from_secs(30);
const INSTRUCTION: &str = r#"Predict the single message the human user is most likely to type next in this conversation. Use the user's language, voice, and level of detail. This is a prediction task. Use earlier messages as evidence. A concrete confirmation of the agreed choice or a specific acknowledgement of completion can be the right prediction. Avoid generic thanks and empty acknowledgements. Do not answer the conversation or call tools. If several continuations are equally plausible, or the next message would invent new facts, preferences, or authorization, return {"suggestion":null}. Otherwise return only {"suggestion":"one short, natural user message"} as JSON."#;

/// In-memory only. Omits connection credentials and only exposes the inference operation.
pub struct AgentPromptPredictionSeed {
    request: LlmChatRequest,
    endpoint_digest: [u8; 32],
    estimated_bytes: usize,
    context_window_tokens: Option<u32>,
}

pub struct AgentPromptPredictionOutput {
    pub suggestion: Option<String>,
    pub usage: Option<AgentUsage>,
}

impl std::fmt::Debug for AgentPromptPredictionOutput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentPromptPredictionOutput")
            .field("has_suggestion", &self.suggestion.is_some())
            .field("usage", &self.usage)
            .finish()
    }
}

impl AgentPromptPredictionSeed {
    pub(super) fn from_final_request(
        mut request: LlmChatRequest,
        final_turn: LlmAssistantTurn,
        context_window_tokens: Option<u32>,
    ) -> Self {
        request
            .messages
            .push(LlmMessage::from_assistant_turn(final_turn));
        Self::new(request, context_window_tokens)
    }

    fn new(mut request: LlmChatRequest, context_window_tokens: Option<u32>) -> Self {
        let endpoint_digest = Sha256::digest(request.api_url.trim().as_bytes()).into();
        request.api_token.zeroize();
        request.api_url.zeroize();
        request
            .messages
            .push(LlmMessage::text(LlmMessageRole::User, INSTRUCTION));
        request.max_tokens = Some(
            request
                .max_tokens
                .unwrap_or(OUTPUT_TOKENS)
                .min(OUTPUT_TOKENS),
        );
        let estimated_bytes = estimate_request_bytes(&request);
        Self {
            request,
            endpoint_digest,
            estimated_bytes,
            context_window_tokens,
        }
    }

    /// Approximate retained size for Host admission, including image bodies and private replay.
    pub fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }

    /// Executes at most one provider attempt. Invalid output, tool calls, and exhausted
    /// capacity yield no candidate. Usage from successful responses survives output rejection.
    pub async fn predict(
        &self,
        api_url: &str,
        api_token: &str,
        cancellation: AgentCancellationToken,
    ) -> AgentResult<AgentPromptPredictionOutput> {
        self.predict_observed(api_url, api_token, cancellation, || {})
            .await
    }

    /// Reports an HTTP dispatch only after local validation, preparation, and provider cooldown.
    pub async fn predict_observed(
        &self,
        api_url: &str,
        api_token: &str,
        cancellation: AgentCancellationToken,
        on_dispatch: impl FnOnce() + Send,
    ) -> AgentResult<AgentPromptPredictionOutput> {
        cancellation.check()?;
        let endpoint_digest: [u8; 32] = Sha256::digest(api_url.trim().as_bytes()).into();
        if endpoint_digest != self.endpoint_digest {
            return Err(AgentError::new("Prompt prediction endpoint changed."));
        }
        if !self.fits_context() {
            return Ok(AgentPromptPredictionOutput {
                suggestion: None,
                usage: None,
            });
        }
        let mut request = self.request.clone();
        request.api_url = api_url.trim().into();
        request.api_token = api_token.trim().into();
        let response =
            complete_prompt_prediction_once(&request, cancellation, DEADLINE, on_dispatch).await?;
        let suggestion = if response.provider_tool_calls().is_empty() {
            parse_suggestion(response.content())
        } else {
            None
        };
        Ok(AgentPromptPredictionOutput {
            suggestion,
            usage: response.usage,
        })
    }

    fn fits_context(&self) -> bool {
        let Some(window) = self.context_window_tokens else {
            return true;
        };
        let detector = ContextCapacityDetector::for_model(
            self.request.model(),
            self.request.api_style(),
            &self.request.tools,
        );
        let overhead = detector.inspect(
            &mut ContextFrame::new(Vec::new()),
            Some(window),
            self.request.max_tokens.unwrap_or(OUTPUT_TOKENS),
        );
        let estimated = self
            .request
            .messages
            .iter()
            .fold(overhead.usage.request_input_tokens(), |tokens, message| {
                tokens.saturating_add(detector.estimate_message_tokens(message))
            });
        overhead
            .available_input_tokens
            .is_some_and(|capacity| estimated <= capacity)
    }
}

fn estimate_request_bytes(request: &LlmChatRequest) -> usize {
    fn json_bytes(value: &Value) -> usize {
        size_of::<Value>()
            + match value {
                Value::String(s) => s.capacity(),
                Value::Object(map) => map
                    .iter()
                    .map(|(k, v)| size_of::<String>() + k.capacity() + json_bytes(v) + 32)
                    .sum(),
                Value::Array(items) => items.iter().map(json_bytes).sum(),
                _ => 0,
            }
    }
    fn call_bytes(call: &crate::llm::LlmToolCall) -> usize {
        size_of_val(call) + call.id.capacity() + call.name.capacity() + json_bytes(&call.args)
    }
    let mut bytes = size_of::<AgentPromptPredictionSeed>()
        + request.messages.capacity() * size_of::<LlmMessage>()
        + request.tools.capacity() * size_of::<AgentToolDefinition>()
        + request.provider_protocol.model_id.len();
    for message in &request.messages {
        if let Some(turn) = message.assistant_turn() {
            bytes += size_of_val(turn)
                + turn.provider_visible_text().len()
                + turn.runtime_visible_text().map_or(0, str::len)
                + turn
                    .provider_tool_calls()
                    .iter()
                    .map(call_bytes)
                    .sum::<usize>()
                + turn
                    .provider_continuation()
                    .map_or(0, |c| c.encoded_bytes())
                + turn
                    .reasoning()
                    .iter()
                    .map(|r| size_of_val(r) + r.summary_text().len())
                    .sum::<usize>();
            for binding in turn.runtime_tool_bindings().unwrap_or_default() {
                bytes += size_of_val(binding)
                    + binding.provider_call_id.len()
                    + call_bytes(&binding.runtime_call);
            }
        } else {
            bytes += message.content().len() + message.tool_call_id().map_or(0, str::len);
        }
        for image in message.images() {
            bytes += size_of_val(image) + image.mime_type.capacity() + image.data_base64.capacity();
        }
    }
    for tool in &request.tools {
        bytes +=
            tool.name.capacity() + tool.description.capacity() + json_bytes(&tool.input_schema);
    }
    bytes
}

fn parse_suggestion(raw: &str) -> Option<String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Prediction {
        #[serde(deserialize_with = "crate::protocol::deserialize_required_nullable")]
        suggestion: Option<String>,
    }
    let output: Prediction = serde_json::from_str(raw).ok()?;
    let text = output.suggestion?;
    let text = text.trim();
    (!text.is_empty() && text.chars().count() <= 200 && !text.chars().any(char::is_control))
        .then(|| text.to_owned())
}

/// Reprojects committed conversation history using the normal host-authorized context and
/// tool projection. The Host supplies the latest model history, including the final answer.
/// Unlike the live seed, this uses currently available request-only capability guidance.
pub fn prepare_prompt_prediction_seed(
    mut input: AgentChatInput,
    conversation_id: &str,
    host: &AgentRuntimeHostServices,
    projection: &AgentContextWindowToolProjection,
) -> AgentResult<AgentPromptPredictionSeed> {
    input.api_token.zeroize();
    let api_url = input.api_url.clone();
    let window = input.context_window_tokens;
    let max_tokens = input.max_tokens;
    let temperature = sanitize_temperature(input.temperature);
    let stream = input.stream.unwrap_or(false);
    let discovery = input.skill_discovery.take();
    let activation = input.skill_activation.take();
    let HydratedConversationContext {
        mut state,
        mut stable_tools,
        profile,
        protocol,
    } = hydrate_conversation_context(input, conversation_id, host)?;
    let frame =
        state.projected_request_frame(discovery.as_ref(), activation.as_ref(), projection)?;
    stable_tools.extend_from_slice(projection.dynamic_definitions());
    let request = LlmChatRequest {
        api_url,
        api_token: String::new(),
        provider_profile: profile,
        provider_protocol: protocol,
        max_tokens,
        temperature,
        stream,
        messages: frame.into_model_request_messages(),
        tools: stable_tools,
    };
    Ok(AgentPromptPredictionSeed::new(request, window))
}

#[cfg(test)]
mod tests {
    use super::parse_suggestion;
    #[test]
    fn only_a_single_json_suggestion_is_accepted() {
        for value in [
            r#"{"suggestion":null}"#,
            r#"{"suggestion":" "}"#,
            r#"{"suggestion":"first\nsecond"}"#,
            r#"{"suggestion":"first", "also":"another"}"#,
            r#"{"suggestion":"a","suggestion":"b"}"#,
            r#"```json {"suggestion":"a"} ```"#,
            r#"{"suggestion": ["a", "b"]}"#,
            "OK",
        ] {
            assert_eq!(parse_suggestion(value), None, "{value}");
        }
        assert_eq!(
            parse_suggestion(r#"{"suggestion":"试试 \"debug\"。"}"#).as_deref(),
            Some("试试 \"debug\"。")
        );
    }
}
