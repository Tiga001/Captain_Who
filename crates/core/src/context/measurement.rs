//! Token measurement strategies and additive message-level estimates.
//!
//! A strategy identity must change whenever its tokenization behavior or configuration changes.
//! `ContextFrame` uses that identity to invalidate item caches safely when a run switches to a
//! different tokenizer. Persisted checkpoints intentionally exclude these derived measurements.

use crate::llm::LlmMessage;
use crate::protocol::AgentToolDefinition;
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
    pub(crate) image_tokens: u64,
    pub(crate) image_count: usize,
}

impl ContextMessageEstimate {
    pub(crate) fn total_tokens(self) -> u64 {
        self.message_content_tokens
            .saturating_add(self.message_structure_tokens)
            .saturating_add(self.tool_call_tokens)
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

    pub(crate) fn estimate(&self, value: &str) -> u64 {
        self.estimator.estimate_text(value)
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
        let message_content_tokens = self.estimate_text(&message.content);
        let mut message_structure_tokens =
            MESSAGE_STRUCTURE_TOKENS.saturating_add(self.estimate_text(message.role.as_str()));
        if let Some(tool_call_id) = &message.tool_call_id {
            message_structure_tokens =
                message_structure_tokens.saturating_add(self.estimate_text(tool_call_id));
        }
        let tool_call_tokens = message.tool_calls.iter().fold(0_u64, |total, call| {
            total
                .saturating_add(TOOL_CALL_STRUCTURE_TOKENS)
                .saturating_add(self.estimate_text(&call.id))
                .saturating_add(self.estimate_text(&call.name))
                .saturating_add(estimate_json_tokens(&call.args))
        });
        let image_count = message.images.len();
        let image_tokens = u64::try_from(image_count)
            .unwrap_or(u64::MAX)
            .saturating_mul(IMAGE_TOKEN_RESERVE);

        ContextMessageEstimate {
            message_content_tokens,
            message_structure_tokens,
            tool_call_tokens,
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

    #[test]
    fn text_budget_returns_a_valid_utf8_prefix() {
        let budget = ContextTextBudget::heuristic(4);
        let value = "abc甲乙丙";

        let prefix = &value[..budget.fitting_prefix_len(value)];

        assert_eq!(prefix, "abc甲");
        assert!(budget.fits(prefix));
        assert!(!budget.fits(value));
    }
}
