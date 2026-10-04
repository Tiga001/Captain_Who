//! Request limits and context reservations are deliberately different concerns.
//! A reservation never becomes a wire limit just because the provider default is unknown.

use crate::protocol::{AgentApiStyle, AgentChatInput, AgentError, AgentResult};
use crate::provider_profile::ProviderProfileConfig;
use crate::provider_registration::resolve_provider_registration;

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

    let provider_reservation = match profile {
        Some(profile) => {
            let registration = resolve_provider_registration(profile.profile(), api_style.into())
                .map_err(|error| {
                AgentError::new(format!(
                    "Provider profile configuration is invalid: {error}"
                ))
            })?;
            registration
                .runtime_capabilities()
                .output_token_reservation(
                    profile.reasoning_mode(),
                    profile.provider_reasoning_effort(),
                )
        }
        None => None,
    };
    Ok(OutputBudget {
        request_max_tokens: None,
        // Unknown defaults remain estimates, not a guaranteed provider output allowance.
        reserved_output_tokens: provider_reservation.unwrap_or(UNKNOWN_PROVIDER_OUTPUT_RESERVE),
    })
}
