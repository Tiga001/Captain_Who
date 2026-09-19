//! Request limits and context reservations are deliberately different concerns.
//! A reservation never becomes a wire limit just because the provider default is unknown.

use crate::protocol::{AgentApiStyle, AgentChatInput, AgentError, AgentResult};
use crate::provider_profile::{
    ProviderFamilySettings, ProviderProfileConfig, ProviderReasoningEffort, ReasoningMode,
};

/// Compatibility estimate only; this is NOT a default limit for chat requests.
const UNKNOWN_PROVIDER_OUTPUT_RESERVE: u32 = 30_000;
/// Messages requires max_tokens. Keep the existing compatibility value for the generic
/// Anthropic adapter rather than guessing a model capability from a user-defined model name.
const ANTHROPIC_COMPATIBILITY_MAX_TOKENS: u32 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputBudget {
    pub(crate) request_max_tokens: Option<u32>,
    pub(crate) reserved_output_tokens: u32,
}

/// Used by request preparation, idle preview and configuration fingerprints alike. The input
/// contains the Host-selected (or resumed, frozen) profile, not a mutable model catalogue.
pub(super) fn resolve_output_budget(
    input: &AgentChatInput,
    api_style: AgentApiStyle,
) -> AgentResult<OutputBudget> {
    resolve_profile_output_budget(
        input.provider_profile_config.as_ref(),
        api_style,
        input.max_tokens,
    )
}

/// Shared by runtime requests and save-time validation of the Host-resolved profile.
pub(crate) fn resolve_profile_output_budget(
    profile: Option<&ProviderProfileConfig>,
    api_style: AgentApiStyle,
    explicit_max_tokens: Option<u32>,
) -> AgentResult<OutputBudget> {
    if let Some(max_tokens) = explicit_max_tokens {
        if max_tokens == 0 {
            return Err(AgentError::structured(
                "agent.invalid_output_limit",
                "模型输出上限必须大于零。",
                serde_json::json!({ "maxTokens": max_tokens }),
            ));
        }
        // Preserve explicit internal/legacy limits exactly. In particular, do not silently
        // clamp newer provider limits to the former universal 128,000 ceiling.
        return Ok(OutputBudget {
            request_max_tokens: Some(max_tokens),
            reserved_output_tokens: max_tokens,
        });
    }
    if api_style == AgentApiStyle::AnthropicCompatible {
        return Ok(OutputBudget {
            request_max_tokens: Some(ANTHROPIC_COMPATIBILITY_MAX_TOKENS),
            reserved_output_tokens: ANTHROPIC_COMPATIBILITY_MAX_TOKENS,
        });
    }

    let reserved_output_tokens = match profile.and_then(|profile| profile.family_settings()) {
        // Verified 2026-09-19: https://api-docs.deepseek.com/api/create-chat-completion/
        // Thinking is enabled by default. These budgets include reasoning, not just text.
        Some(
            ProviderFamilySettings::DeepseekFlashChat { reasoning }
            | ProviderFamilySettings::DeepseekProChat { reasoning },
        ) => {
            // Profile validation rejects disabled thinking with a non-default effort.
            if reasoning.mode == ReasoningMode::Disabled {
                8 * 1024
            } else if reasoning.effort == ProviderReasoningEffort::Max {
                128 * 1024
            } else {
                64 * 1024
            }
        }
        // https://www.kimi.com/help/kimi-api/api-troubleshooting documents K3's default.
        Some(ProviderFamilySettings::MoonshotK3Chat { .. }) => 128 * 1024,
        // Unknown provider defaults remain estimates. Never infer a family from model name
        // substrings or advertise this estimate as a guaranteed provider output allowance.
        _ => UNKNOWN_PROVIDER_OUTPUT_RESERVE,
    };
    Ok(OutputBudget {
        request_max_tokens: None,
        reserved_output_tokens,
    })
}
