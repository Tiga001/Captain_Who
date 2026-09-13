//! Token measurement strategies and additive message-level estimates.
//!
//! A strategy identity must change whenever its tokenization behavior or configuration changes.
//! `ContextFrame` uses that identity to invalidate item caches safely when a run switches to a
//! different tokenizer. Persisted checkpoints intentionally exclude these derived measurements.

use crate::llm::{LlmAssistantTurn, LlmMessage, LlmMessageRole};
use crate::protocol::AgentToolDefinition;
use crate::provider_profile::ProviderProtocolKey;
use std::fmt::Debug;
use std::sync::Arc;

const HEURISTIC_ESTIMATOR_ID: &str = "unicode_heuristic";
const HEURISTIC_ESTIMATOR_VERSION: u32 = 1;
const ASCII_CHARACTERS_PER_TOKEN: u64 = 3;
const NON_ASCII_TOKENS_PER_CHARACTER: u64 = 2;
const REQUEST_STRUCTURE_TOKENS: u64 = 16;
const MESSAGE_STRUCTURE_TOKENS: u64 = 8;
const TOOL_CALL_STRUCTURE_TOKENS: u64 = 12;
const IMAGE_TOKEN_RESERVE: u64 = 4_096;
const CONTEXT_REVISION_FNV_OFFSET: u64 = 0xcbf29ce484222325;
const CONTEXT_REVISION_FNV_PRIME: u64 = 0x100000001b3;

fn context_projection_semantics(
    protocol: Option<&ProviderProtocolKey>,
) -> crate::ProviderContextProjectionSemantics {
    let Some(protocol) = protocol else {
        return crate::ProviderContextProjectionSemantics::GenericEffectiveCalls;
    };
    crate::resolve_provider_runtime_capabilities(protocol)
        .map(|capabilities| capabilities.context_projection())
        // Measurement cannot return an error. Treat an unregistered protocol as the richer exact
        // projection so this diagnostic path never silently undercounts it as Generic; request
        // admission still rejects the unsupported registration before any provider call.
        .unwrap_or(crate::ProviderContextProjectionSemantics::ExactProviderTurn)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ContextRevisionHasher {
    state: u64,
}

impl ContextRevisionHasher {
    pub(crate) fn new() -> Self {
        Self {
            state: CONTEXT_REVISION_FNV_OFFSET,
        }
    }

    pub(crate) fn write_u64(&mut self, value: u64) {
        self.write_raw(&value.to_le_bytes());
    }

    pub(crate) fn write_str(&mut self, value: &str) {
        self.write_u64(u64::try_from(value.len()).unwrap_or(u64::MAX));
        self.write_raw(value.as_bytes());
    }

    pub(crate) fn finish(self) -> u64 {
        self.state
    }

    fn write_raw(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.state = (self.state ^ u64::from(*byte)).wrapping_mul(CONTEXT_REVISION_FNV_PRIME);
        }
    }
}

pub(crate) fn combine_context_revisions(left: u64, right: u64) -> u64 {
    let mut hasher = ContextRevisionHasher::new();
    hasher.write_u64(left);
    hasher.write_u64(right);
    hasher.finish()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextEstimatorIdentity {
    pub(crate) family: &'static str,
    pub(crate) version: u32,
    /// Optional tokenizer vocabulary/model variant. `Arc` keeps per-item cache clones cheap.
    pub(crate) variant: Option<Arc<str>>,
}

impl ContextEstimatorIdentity {
    pub(crate) fn label(&self) -> String {
        self.variant.as_ref().map_or_else(
            || self.family.to_string(),
            |variant| format!("{}:{variant}", self.family),
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ContextMessageEstimate {
    pub(crate) message_content_tokens: u64,
    pub(crate) message_structure_tokens: u64,
    pub(crate) tool_call_tokens: u64,
    /// Provider-owned opaque continuation bytes that will be projected onto the wire. Kept
    /// separate from Tool Call JSON so diagnostics can identify hidden protocol-state cost.
    pub(crate) provider_continuation_tokens: u64,
    pub(crate) image_tokens: u64,
    pub(crate) image_count: usize,
}

impl ContextMessageEstimate {
    pub(crate) fn total_tokens(self) -> u64 {
        self.message_content_tokens
            .saturating_add(self.message_structure_tokens)
            .saturating_add(self.tool_call_tokens)
            .saturating_add(self.provider_continuation_tokens)
            .saturating_add(self.image_tokens)
    }

    pub(crate) fn merge(&mut self, other: Self) {
        self.message_content_tokens = self
            .message_content_tokens
            .saturating_add(other.message_content_tokens);
        self.message_structure_tokens = self
            .message_structure_tokens
            .saturating_add(other.message_structure_tokens);
        self.tool_call_tokens = self.tool_call_tokens.saturating_add(other.tool_call_tokens);
        self.provider_continuation_tokens = self
            .provider_continuation_tokens
            .saturating_add(other.provider_continuation_tokens);
        self.image_tokens = self.image_tokens.saturating_add(other.image_tokens);
        self.image_count = self.image_count.saturating_add(other.image_count);
    }
}

pub(crate) trait ContextTokenEstimator: Debug + Send + Sync {
    fn identity(&self) -> ContextEstimatorIdentity;

    /// Estimates standalone text with the same tokenizer family used for request accounting.
    ///
    /// Tool output budgets use this hook so a future provider tokenizer can replace the current
    /// heuristic without introducing a second, inconsistent measurement path.
    fn estimate_text(&self, value: &str) -> u64 {
        estimate_text_tokens(value)
    }

    /// Selects a UTF-8 prefix that fits a standalone text allowance. Tokenizers whose prefix
    /// counts are not monotonic can override this with a native incremental implementation.
    fn fitting_text_prefix_len(&self, value: &str, max_tokens: u64) -> usize {
        fitting_prefix_len_by_estimate(value, max_tokens, |prefix| self.estimate_text(prefix))
    }

    /// Fast per-message measurement used by the incremental frame cache.
    fn estimate_message(&self, message: &LlmMessage) -> ContextMessageEstimate;

    /// Whole-frame verification hook for tokenizers whose boundary behavior is not additive.
    fn estimate_messages(&self, messages: &[&LlmMessage]) -> ContextMessageEstimate {
        messages
            .iter()
            .fold(ContextMessageEstimate::default(), |mut total, message| {
                total.merge(self.estimate_message(message));
                total
            })
    }

    fn estimate_tool_definitions(&self, tools: &[AgentToolDefinition]) -> u64;

    fn request_structure_tokens(&self) -> u64;

    fn image_token_reserve_per_image(&self) -> u64;

    /// `None` means per-item estimates are exactly additive. Boundary-sensitive tokenizers may
    /// request a whole-frame verification after this percentage of available input is reached.
    fn full_recount_threshold_percent(&self) -> Option<u8>;
}

/// An immutable text allowance derived from the request's context capacity policy.
///
/// This value deliberately carries the estimator as well as the numeric limit. Consumers can
/// therefore test and trim text with the exact same measurement family that protects the model
/// request, while remaining unaware of model-window policy.
#[derive(Debug, Clone)]
pub(crate) struct ContextTextBudget {
    estimator: Arc<dyn ContextTokenEstimator>,
    max_tokens: u64,
}

impl ContextTextBudget {
    pub(crate) const DEFAULT_MAX_TOKENS: u64 = 24_000;

    pub(crate) fn new(estimator: Arc<dyn ContextTokenEstimator>, max_tokens: u64) -> Self {
        Self {
            estimator,
            max_tokens: max_tokens.max(1),
        }
    }

    pub(crate) fn heuristic(max_tokens: u64) -> Self {
        Self::new(Arc::new(HeuristicTokenEstimator), max_tokens)
    }

    pub(crate) fn heuristic_default() -> Self {
        Self::heuristic(Self::DEFAULT_MAX_TOKENS)
    }

    pub(crate) fn max_tokens(&self) -> u64 {
        self.max_tokens
    }

    pub(crate) fn with_max_tokens(&self, max_tokens: u64) -> Self {
        Self::new(self.estimator.clone(), max_tokens)
    }

    pub(crate) fn estimate(&self, value: &str) -> u64 {
        self.estimator.estimate_text(value)
    }

    /// Measures a complete provider-facing message with the same estimator as the owning
    /// request. Runtime extensions use this for retained protocol messages whose structure,
    /// tool-call ids, or tool arguments cannot be represented by a plain text allowance.
    pub(crate) fn estimate_message(&self, message: &LlmMessage) -> u64 {
        self.estimator.estimate_message(message).total_tokens()
    }

    /// Measures model-facing Tool schemas with the same estimator as the owning request.
    ///
    /// Dynamic Skill activation uses this to reserve only the incremental schema cost of the
    /// projected post-activation Tool set before publishing any activation side effects.
    pub(crate) fn estimate_tool_definitions(&self, tools: &[AgentToolDefinition]) -> u64 {
        self.estimator.estimate_tool_definitions(tools)
    }

    pub(crate) fn fits(&self, value: &str) -> bool {
        self.estimate(value) <= self.max_tokens
    }

    /// Returns the largest UTF-8 prefix that fits the allowance.
    pub(crate) fn fitting_prefix_len(&self, value: &str) -> usize {
        self.estimator
            .fitting_text_prefix_len(value, self.max_tokens)
    }
}

fn fitting_prefix_len_by_estimate(
    value: &str,
    max_tokens: u64,
    estimate: impl Fn(&str) -> u64,
) -> usize {
    if value.is_empty() || estimate(value) <= max_tokens {
        return value.len();
    }

    let mut boundaries = Vec::with_capacity(value.chars().count().saturating_add(1));
    boundaries.push(0);
    boundaries.extend(value.char_indices().skip(1).map(|(index, _)| index));
    boundaries.push(value.len());

    let mut fitting = 0_usize;
    let mut rejected = boundaries.len().saturating_sub(1);
    while fitting.saturating_add(1) < rejected {
        let candidate = fitting + (rejected - fitting) / 2;
        if estimate(&value[..boundaries[candidate]]) <= max_tokens {
            fitting = candidate;
        } else {
            rejected = candidate;
        }
    }
    boundaries[fitting]
}

#[derive(Debug, Default)]
pub(crate) struct HeuristicTokenEstimator;

impl ContextTokenEstimator for HeuristicTokenEstimator {
    fn identity(&self) -> ContextEstimatorIdentity {
        ContextEstimatorIdentity {
            family: HEURISTIC_ESTIMATOR_ID,
            version: HEURISTIC_ESTIMATOR_VERSION,
            variant: None,
        }
    }

    fn estimate_message(&self, message: &LlmMessage) -> ContextMessageEstimate {
        let exact_provider_turn = message.assistant_turn().filter(|turn| {
            context_projection_semantics(turn.provider_protocol())
                == crate::ProviderContextProjectionSemantics::ExactProviderTurn
        });
        let message_content_tokens = self.estimate_text(
            exact_provider_turn
                .map(LlmAssistantTurn::provider_visible_text)
                .unwrap_or_else(|| message.content()),
        );
        let mut message_structure_tokens =
            MESSAGE_STRUCTURE_TOKENS.saturating_add(self.estimate_text(message.role().as_str()));
        if let Some(tool_call_id) = message.tool_call_id() {
            message_structure_tokens =
                message_structure_tokens.saturating_add(self.estimate_text(tool_call_id));
        }
        let mut tool_call_tokens = if let Some(turn) = exact_provider_turn {
            turn.provider_tool_calls()
                .iter()
                .fold(0_u64, |total, call| {
                    total
                        .saturating_add(TOOL_CALL_STRUCTURE_TOKENS)
                        .saturating_add(self.estimate_text(&call.id))
                        .saturating_add(self.estimate_text(&call.name))
                        .saturating_add(estimate_json_tokens(&call.args))
                })
        } else {
            message.tool_calls().fold(0_u64, |total, call| {
                total
                    .saturating_add(TOOL_CALL_STRUCTURE_TOKENS)
                    .saturating_add(self.estimate_text(&call.id))
                    .saturating_add(self.estimate_text(&call.name))
                    .saturating_add(estimate_json_tokens(&call.args))
            })
        };
        if let Some(turn) = exact_provider_turn {
            // Exact provider projections replay the original provider call id. Context stores the
            // bounded runtime id, so reserve the positive difference here for every result.
            for binding in turn.runtime_tool_bindings().unwrap_or_default() {
                let provider_id_tokens = self.estimate_text(&binding.provider_call_id);
                let runtime_id_tokens = self.estimate_text(&binding.runtime_call.id);
                tool_call_tokens = tool_call_tokens
                    .saturating_add(provider_id_tokens.saturating_sub(runtime_id_tokens));
            }
        }
        if let Some(turn) = message.assistant_turn() {
            let effective_call_count = turn.effective_tool_calls().len();
            let uses_generic_split_projection =
                context_projection_semantics(turn.provider_protocol())
                    == crate::ProviderContextProjectionSemantics::GenericEffectiveCalls;
            if turn.runtime_tool_bindings().is_some()
                && uses_generic_split_projection
                && effective_call_count > 1
            {
                let extra_assistant_messages =
                    u64::try_from(effective_call_count - 1).unwrap_or(u64::MAX);
                let per_message = MESSAGE_STRUCTURE_TOKENS
                    .saturating_add(self.estimate_text(LlmMessageRole::Assistant.as_str()));
                message_structure_tokens = message_structure_tokens
                    .saturating_add(extra_assistant_messages.saturating_mul(per_message));
            }
        }
        let provider_continuation_tokens = message.assistant_turn().map_or(0, |turn| {
            crate::llm::estimate_assistant_turn_continuation_tokens(turn).unwrap_or(u64::MAX)
        });
        let image_count = message.images().len();
        let image_tokens = u64::try_from(image_count)
            .unwrap_or(u64::MAX)
            .saturating_mul(IMAGE_TOKEN_RESERVE);

        ContextMessageEstimate {
            message_content_tokens,
            message_structure_tokens,
            tool_call_tokens,
            provider_continuation_tokens,
            image_tokens,
            image_count,
        }
    }

    fn estimate_tool_definitions(&self, tools: &[AgentToolDefinition]) -> u64 {
        tools.iter().fold(0_u64, |total, tool| {
            total.saturating_add(
                serde_json::to_string(tool)
                    .ok()
                    .map_or(0, |value| self.estimate_text(&value)),
            )
        })
    }

    fn request_structure_tokens(&self) -> u64 {
        REQUEST_STRUCTURE_TOKENS
    }

    fn image_token_reserve_per_image(&self) -> u64 {
        IMAGE_TOKEN_RESERVE
    }

    fn full_recount_threshold_percent(&self) -> Option<u8> {
        None
    }
}

fn estimate_text_tokens(value: &str) -> u64 {
    let (ascii_characters, non_ascii_characters) =
        value.chars().fold((0_u64, 0_u64), |counts, character| {
            if character.is_ascii() {
                (counts.0.saturating_add(1), counts.1)
            } else {
                (counts.0, counts.1.saturating_add(1))
            }
        });
    ascii_characters
        .div_ceil(ASCII_CHARACTERS_PER_TOKEN)
        .saturating_add(non_ascii_characters.saturating_mul(NON_ASCII_TOKENS_PER_CHARACTER))
}

fn estimate_json_tokens(value: &serde_json::Value) -> u64 {
    serde_json::to_string(value)
        .ok()
        .map_or(0, |serialized| estimate_text_tokens(&serialized))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{LlmAssistantTurn, LlmRuntimeToolCallBinding, LlmToolCall};
    use crate::provider_profile::{
        ProviderProfileConfig, ProviderProfileId, ProviderProfileRef, ProviderProtocolDialect,
        ProviderProtocolKey,
    };
    use serde_json::json;

    #[test]
    fn text_budget_returns_a_valid_utf8_prefix() {
        let budget = ContextTextBudget::heuristic(4);
        let value = "abc甲乙丙";

        let prefix = &value[..budget.fitting_prefix_len(value)];

        assert_eq!(prefix, "abc甲");
        assert!(budget.fits(prefix));
        assert!(!budget.fits(value));
    }

    #[test]
    fn complete_generic_turn_matches_split_wire_message_budget() {
        let calls = (0..3)
            .map(|index| LlmToolCall {
                id: format!("call-{index}"),
                name: "read_file".to_string(),
                args: json!({ "path": format!("file-{index}.txt") }),
            })
            .collect::<Vec<_>>();
        let bindings = calls
            .iter()
            .enumerate()
            .map(|(index, call)| LlmRuntimeToolCallBinding::new(index, call, call.clone()))
            .collect();
        let complete = LlmMessage::from_assistant_turn(
            LlmAssistantTurn::from_split_projection("I will inspect the files.", calls.clone())
                .with_runtime_tool_bindings(bindings)
                .unwrap(),
        );
        let split = calls
            .into_iter()
            .enumerate()
            .map(|(index, call)| {
                LlmMessage::assistant(
                    if index == 0 {
                        "I will inspect the files."
                    } else {
                        ""
                    },
                    vec![call],
                )
            })
            .collect::<Vec<_>>();
        let estimator = HeuristicTokenEstimator;
        let complete_tokens = estimator.estimate_message(&complete).total_tokens();
        let split_tokens = split
            .iter()
            .map(|message| estimator.estimate_message(message).total_tokens())
            .sum::<u64>();

        assert_eq!(complete_tokens, split_tokens);
    }

    #[test]
    fn grouped_generic_turn_keeps_single_assistant_structure_cost() {
        let calls = vec![
            LlmToolCall {
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                args: json!({ "path": "one.txt" }),
            },
            LlmToolCall {
                id: "call-2".to_string(),
                name: "read_file".to_string(),
                args: json!({ "path": "two.txt" }),
            },
        ];
        let historical_grouped = LlmMessage::assistant("", calls.clone());
        let bindings = calls
            .iter()
            .enumerate()
            .map(|(index, call)| LlmRuntimeToolCallBinding::new(index, call, call.clone()))
            .collect();
        let runtime_split = LlmMessage::from_assistant_turn(
            LlmAssistantTurn::from_split_projection("", calls)
                .with_runtime_tool_bindings(bindings)
                .unwrap(),
        );
        let estimator = HeuristicTokenEstimator;
        let single_assistant_structure =
            MESSAGE_STRUCTURE_TOKENS + estimator.estimate_text(LlmMessageRole::Assistant.as_str());

        assert_eq!(
            estimator.estimate_message(&runtime_split).total_tokens(),
            estimator
                .estimate_message(&historical_grouped)
                .total_tokens()
                + single_assistant_structure
        );
    }

    #[test]
    fn grouped_provider_profile_does_not_pay_generic_split_overhead() {
        let profile = ProviderProfileConfig::deepseek_flash_default();
        let key = ProviderProtocolKey::new(
            ProviderProtocolDialect::OpenAiChatCompletions,
            &profile,
            "deepseek-flash",
            None,
        )
        .unwrap();
        let provider_calls = vec![
            LlmToolCall {
                id: "provider-1".to_string(),
                name: "read_file".to_string(),
                args: json!({ "path": "one" }),
            },
            LlmToolCall {
                id: "provider-2".to_string(),
                name: "read_file".to_string(),
                args: json!({ "path": "two" }),
            },
        ];
        let bindings = provider_calls
            .iter()
            .enumerate()
            .map(|(index, call)| LlmRuntimeToolCallBinding::new(index, call, call.clone()))
            .collect::<Vec<_>>();
        let turn = LlmAssistantTurn::from_provider(key, "", provider_calls.clone())
            .unwrap()
            .with_runtime_tool_bindings(bindings.clone())
            .unwrap();
        let grouped = LlmMessage::from_assistant_turn(turn);
        let generic_live = LlmMessage::from_assistant_turn(
            LlmAssistantTurn::from_split_projection("", provider_calls)
                .with_runtime_tool_bindings(bindings)
                .unwrap(),
        );
        let estimator = HeuristicTokenEstimator;

        assert!(
            estimator.estimate_message(&grouped).total_tokens()
                < estimator.estimate_message(&generic_live).total_tokens()
        );
    }

    #[test]
    fn unknown_provider_registration_uses_conservative_exact_projection_for_measurement() {
        let unsupported = ProviderProtocolKey {
            dialect: ProviderProtocolDialect::OpenAiChatCompletions,
            profile: ProviderProfileRef {
                id: ProviderProfileId::DeepSeekV41FlashChat,
                version: u32::MAX,
            },
            model_id: "unsupported-provider-version".to_string(),
            provider_configuration_revision: None,
        };

        assert_eq!(
            context_projection_semantics(Some(&unsupported)),
            crate::ProviderContextProjectionSemantics::ExactProviderTurn
        );
    }

    #[test]
    fn deepseek_estimate_uses_original_provider_calls_and_result_ids() {
        let profile = ProviderProfileConfig::deepseek_flash_default();
        let key = ProviderProtocolKey::new(
            ProviderProtocolDialect::OpenAiChatCompletions,
            &profile,
            "deepseek-flash",
            None,
        )
        .unwrap();
        let provider_call = LlmToolCall {
            id: format!("provider-{}", "x".repeat(8_000)),
            name: "run_command".to_string(),
            args: json!({
                "command": serde_json::to_string(&json!({
                    "script": "\\\"quoted\\\"\n".repeat(1_000)
                })).unwrap()
            }),
        };
        let runtime_call = LlmToolCall {
            id: "runtime-fixed-id".to_string(),
            name: "run_command".to_string(),
            args: json!({"command": "normalized"}),
        };
        let provider_text = "必须仍计入 Provider wire 的可见正文。".repeat(1_000);
        let mut turn = LlmAssistantTurn::from_provider(
            key,
            provider_text.clone(),
            vec![provider_call.clone()],
        )
        .unwrap()
        .with_runtime_tool_bindings(vec![LlmRuntimeToolCallBinding::new(
            0,
            &provider_call,
            runtime_call.clone(),
        )])
        .unwrap();
        turn.set_runtime_visible_text("");
        let deepseek = LlmMessage::from_assistant_turn(turn);
        let runtime_only = LlmMessage::assistant("", vec![runtime_call]);
        let estimator = HeuristicTokenEstimator;
        let deepseek_estimate = estimator.estimate_message(&deepseek);
        let runtime_estimate = estimator.estimate_message(&runtime_only);

        assert!(deepseek_estimate.tool_call_tokens > runtime_estimate.tool_call_tokens + 5_000);
        assert!(
            deepseek_estimate.message_content_tokens >= estimator.estimate_text(&provider_text)
        );
        assert!(
            deepseek_estimate.tool_call_tokens
                >= estimate_json_tokens(&provider_call.args)
                    .saturating_add(estimator.estimate_text(&provider_call.id).saturating_mul(2))
        );
    }
}
