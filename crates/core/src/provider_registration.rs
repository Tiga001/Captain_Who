//! Code-owned Provider Profile registrations and runtime behavior contracts.
//!
//! These values are deliberately not serializable. Persisted settings select a versioned
//! [`ProviderProfileRef`]; the running binary resolves that exact identity and dialect to one
//! immutable registration. Renderer or stored JSON therefore cannot manufacture runtime powers.

use crate::provider_profile::{
    ProviderProfileConfig, ProviderProfileId, ProviderProfilePublicSettings, ProviderProfileRef,
    ProviderProfileValidationError, ProviderProtocolDialect, ProviderProtocolKey, ReasoningMode,
    DEEPSEEK_V4_CHAT_PROFILE_VERSION, GENERIC_ANTHROPIC_MESSAGES_PROFILE_VERSION,
    GENERIC_OPENAI_CHAT_PROFILE_VERSION, PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderToolExchangeSemantics {
    LegacySplit,
    ExactProviderGrouped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderPrivateReplaySemantics {
    None,
    AdapterClassified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderContextProjectionSemantics {
    LegacyEffectiveCalls,
    ExactProviderTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderUsageSemantics {
    StandardAdditive,
    CompletionIncludesReasoning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderPartialTraceSemantics {
    IncrementalBaseline,
    DeferUntilProviderTurnClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderToolCallSourceSemantics {
    LegacyTextFallbackAllowed,
    ProviderNativeOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderTerminalBatchSemantics {
    IndependentCalls,
    CloseWholeProviderTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderSameTurnSkillActivationSemantics {
    FilterUnactivatedSiblings,
    PreserveAndGuardSiblings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderCheckpointPrivateArgumentsSemantics {
    Reject,
    RehydrateFromAuthenticatedTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderContinuationRequirement {
    Forbidden,
    Optional,
    Required,
}

/// Public editor family for one Profile. Unlike Runtime Capabilities this value only selects a
/// strongly typed Renderer component and is safe to project over IPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProfileSettingsKind {
    None,
    DeepseekV4Chat,
}

/// Sanitized, read-only projection of one code-owned Registration.
///
/// Do not add Runtime, replay, checkpoint, usage or continuation policy fields here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderProfileUiDescriptor {
    pub profile_id: ProviderProfileId,
    pub profile_version: u32,
    pub display_name: &'static str,
    pub compatible_dialects: Vec<ProviderProtocolDialect>,
    pub settings_kind: ProviderProfileSettingsKind,
    pub selectable: bool,
}

/// Immutable, code-owned behavior selected by an exact Profile registration.
///
/// Fields stay private so consumers can inspect registered semantics but cannot construct a
/// forged capability set. This type intentionally has no Serde implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProviderRuntimeCapabilities {
    tool_exchange: ProviderToolExchangeSemantics,
    private_replay: ProviderPrivateReplaySemantics,
    context_projection: ProviderContextProjectionSemantics,
    usage: ProviderUsageSemantics,
    partial_trace: ProviderPartialTraceSemantics,
    tool_call_source: ProviderToolCallSourceSemantics,
    terminal_batch: ProviderTerminalBatchSemantics,
    same_turn_skill_activation: ProviderSameTurnSkillActivationSemantics,
    checkpoint_private_arguments: ProviderCheckpointPrivateArgumentsSemantics,
}

impl ProviderRuntimeCapabilities {
    const fn generic() -> Self {
        Self {
            tool_exchange: ProviderToolExchangeSemantics::LegacySplit,
            private_replay: ProviderPrivateReplaySemantics::None,
            context_projection: ProviderContextProjectionSemantics::LegacyEffectiveCalls,
            usage: ProviderUsageSemantics::StandardAdditive,
            partial_trace: ProviderPartialTraceSemantics::IncrementalBaseline,
            tool_call_source: ProviderToolCallSourceSemantics::LegacyTextFallbackAllowed,
            terminal_batch: ProviderTerminalBatchSemantics::IndependentCalls,
            same_turn_skill_activation:
                ProviderSameTurnSkillActivationSemantics::FilterUnactivatedSiblings,
            checkpoint_private_arguments: ProviderCheckpointPrivateArgumentsSemantics::Reject,
        }
    }

    const fn exact_provider_grouped() -> Self {
        Self {
            tool_exchange: ProviderToolExchangeSemantics::ExactProviderGrouped,
            private_replay: ProviderPrivateReplaySemantics::AdapterClassified,
            context_projection: ProviderContextProjectionSemantics::ExactProviderTurn,
            usage: ProviderUsageSemantics::CompletionIncludesReasoning,
            partial_trace: ProviderPartialTraceSemantics::DeferUntilProviderTurnClosed,
            tool_call_source: ProviderToolCallSourceSemantics::ProviderNativeOnly,
            terminal_batch: ProviderTerminalBatchSemantics::CloseWholeProviderTurn,
            same_turn_skill_activation:
                ProviderSameTurnSkillActivationSemantics::PreserveAndGuardSiblings,
            checkpoint_private_arguments:
                ProviderCheckpointPrivateArgumentsSemantics::RehydrateFromAuthenticatedTurn,
        }
    }

    pub const fn tool_exchange(self) -> ProviderToolExchangeSemantics {
        self.tool_exchange
    }

    pub const fn private_replay(self) -> ProviderPrivateReplaySemantics {
        self.private_replay
    }

    pub const fn context_projection(self) -> ProviderContextProjectionSemantics {
        self.context_projection
    }

    pub const fn usage(self) -> ProviderUsageSemantics {
        self.usage
    }

    pub const fn partial_trace(self) -> ProviderPartialTraceSemantics {
        self.partial_trace
    }

    pub const fn tool_call_source(self) -> ProviderToolCallSourceSemantics {
        self.tool_call_source
    }

    pub const fn terminal_batch(self) -> ProviderTerminalBatchSemantics {
        self.terminal_batch
    }

    pub const fn same_turn_skill_activation(self) -> ProviderSameTurnSkillActivationSemantics {
        self.same_turn_skill_activation
    }

    pub const fn checkpoint_private_arguments(self) -> ProviderCheckpointPrivateArgumentsSemantics {
        self.checkpoint_private_arguments
    }

    pub const fn requires_provider_native_tool_calls(self) -> bool {
        matches!(
            self.tool_call_source,
            ProviderToolCallSourceSemantics::ProviderNativeOnly
        )
    }

    pub const fn preserves_skill_activation_batch(self) -> bool {
        matches!(
            self.same_turn_skill_activation,
            ProviderSameTurnSkillActivationSemantics::PreserveAndGuardSiblings
        )
    }

    pub const fn allows_encrypted_checkpoint_rehydration(self) -> bool {
        matches!(
            self.checkpoint_private_arguments,
            ProviderCheckpointPrivateArgumentsSemantics::RehydrateFromAuthenticatedTurn
        )
    }

    pub const fn classify_turn(
        self,
        has_provider_tool_calls: bool,
        has_provider_continuation: bool,
        reasoning_mode: ReasoningMode,
    ) -> ProviderTurnRuntimePolicy {
        let preserves_grouped_turn = has_provider_tool_calls
            && matches!(
                self.tool_exchange,
                ProviderToolExchangeSemantics::ExactProviderGrouped
            );
        let adapter_classifies_private_replay = matches!(
            self.private_replay,
            ProviderPrivateReplaySemantics::AdapterClassified
        );
        let continuation_requirement = if !adapter_classifies_private_replay {
            ProviderContinuationRequirement::Forbidden
        } else if has_provider_tool_calls && matches!(reasoning_mode, ReasoningMode::Enabled) {
            ProviderContinuationRequirement::Required
        } else {
            ProviderContinuationRequirement::Optional
        };
        let requires_private_replay = adapter_classifies_private_replay
            && (has_provider_continuation || has_provider_tool_calls);
        let requires_exact_approval_refs =
            adapter_classifies_private_replay && has_provider_tool_calls;
        let allows_encrypted_checkpoint_rehydration = has_provider_tool_calls
            && matches!(
                self.checkpoint_private_arguments,
                ProviderCheckpointPrivateArgumentsSemantics::RehydrateFromAuthenticatedTurn
            );
        let settles_entire_batch_on_terminal = has_provider_tool_calls
            && matches!(
                self.terminal_batch,
                ProviderTerminalBatchSemantics::CloseWholeProviderTurn
            );

        ProviderTurnRuntimePolicy {
            requires_private_replay,
            continuation_requirement,
            preserves_grouped_turn,
            requires_exact_approval_refs,
            allows_encrypted_checkpoint_rehydration,
            settles_entire_batch_on_terminal,
        }
    }
}

/// Per-turn lifecycle decisions derived from registered capabilities and non-opaque turn facts.
/// This type intentionally has no Serde implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProviderTurnRuntimePolicy {
    requires_private_replay: bool,
    continuation_requirement: ProviderContinuationRequirement,
    preserves_grouped_turn: bool,
    requires_exact_approval_refs: bool,
    allows_encrypted_checkpoint_rehydration: bool,
    settles_entire_batch_on_terminal: bool,
}

impl ProviderTurnRuntimePolicy {
    pub const fn requires_private_replay(self) -> bool {
        self.requires_private_replay
    }

    pub const fn continuation_requirement(self) -> ProviderContinuationRequirement {
        self.continuation_requirement
    }

    pub const fn preserves_grouped_turn(self) -> bool {
        self.preserves_grouped_turn
    }

    pub const fn requires_exact_approval_refs(self) -> bool {
        self.requires_exact_approval_refs
    }

    pub const fn allows_encrypted_checkpoint_rehydration(self) -> bool {
        self.allows_encrypted_checkpoint_rehydration
    }

    pub const fn settles_entire_batch_on_terminal(self) -> bool {
        self.settles_entire_batch_on_terminal
    }
}

/// Exact code registration for one `(profile id, profile version, dialect)` tuple.
/// This type intentionally has no Serde implementation.
#[derive(Debug, PartialEq, Eq)]
pub struct ProviderRegistration {
    profile: ProviderProfileRef,
    dialect: ProviderProtocolDialect,
    runtime_capabilities: ProviderRuntimeCapabilities,
    adapter_kind: ProviderAdapterKind,
    display_name: &'static str,
    settings_kind: ProviderProfileSettingsKind,
    ui_selectable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ProviderAdapterKind {
    GenericOpenAi,
    GenericAnthropic,
    DeepSeekV4Chat,
}

impl ProviderRegistration {
    const fn new(
        profile: ProviderProfileRef,
        dialect: ProviderProtocolDialect,
        runtime_capabilities: ProviderRuntimeCapabilities,
        adapter_kind: ProviderAdapterKind,
        display_name: &'static str,
        settings_kind: ProviderProfileSettingsKind,
        ui_selectable: bool,
    ) -> Self {
        Self {
            profile,
            dialect,
            runtime_capabilities,
            adapter_kind,
            display_name,
            settings_kind,
            ui_selectable,
        }
    }

    pub const fn profile(&self) -> ProviderProfileRef {
        self.profile
    }

    pub const fn dialect(&self) -> ProviderProtocolDialect {
        self.dialect
    }

    pub const fn runtime_capabilities(&self) -> ProviderRuntimeCapabilities {
        self.runtime_capabilities
    }

    pub(crate) const fn adapter_kind(&self) -> ProviderAdapterKind {
        self.adapter_kind
    }

    pub fn ui_descriptor(&self) -> ProviderProfileUiDescriptor {
        ProviderProfileUiDescriptor {
            profile_id: self.profile.id,
            profile_version: self.profile.version,
            display_name: self.display_name,
            compatible_dialects: vec![self.dialect],
            settings_kind: self.settings_kind,
            selectable: self.ui_selectable,
        }
    }

    pub fn config_from_public_settings(
        &self,
        settings: ProviderProfilePublicSettings,
    ) -> Result<ProviderProfileConfig, ProviderProfileValidationError> {
        if !self.ui_selectable {
            return Err(ProviderProfileValidationError::ProfileIsNotUiSelectable {
                id: self.profile.id,
            });
        }
        let settings = settings.normalized();
        if !matches!(
            (self.settings_kind, settings),
            (
                ProviderProfileSettingsKind::DeepseekV4Chat,
                ProviderProfilePublicSettings::DeepseekV4Chat { .. }
            )
        ) {
            return Err(ProviderProfileValidationError::PublicSettingsKindMismatch {
                id: self.profile.id,
            });
        }
        let config = ProviderProfileConfig {
            schema_version: PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
            profile: self.profile,
            reasoning: settings.reasoning(),
        };
        config.validate_for_dialect(self.dialect)?;
        Ok(config)
    }
}

pub(crate) static GENERIC_OPENAI_CHAT_REGISTRATION: ProviderRegistration =
    ProviderRegistration::new(
        ProviderProfileRef {
            id: ProviderProfileId::GenericOpenAiChat,
            version: GENERIC_OPENAI_CHAT_PROFILE_VERSION,
        },
        ProviderProtocolDialect::OpenAiChatCompletions,
        ProviderRuntimeCapabilities::generic(),
        ProviderAdapterKind::GenericOpenAi,
        "通用兼容",
        ProviderProfileSettingsKind::None,
        false,
    );

pub(crate) static GENERIC_ANTHROPIC_MESSAGES_REGISTRATION: ProviderRegistration =
    ProviderRegistration::new(
        ProviderProfileRef {
            id: ProviderProfileId::GenericAnthropicMessages,
            version: GENERIC_ANTHROPIC_MESSAGES_PROFILE_VERSION,
        },
        ProviderProtocolDialect::AnthropicMessages,
        ProviderRuntimeCapabilities::generic(),
        ProviderAdapterKind::GenericAnthropic,
        "通用兼容",
        ProviderProfileSettingsKind::None,
        false,
    );

pub(crate) static DEEPSEEK_V4_CHAT_REGISTRATION: ProviderRegistration = ProviderRegistration::new(
    ProviderProfileRef {
        id: ProviderProfileId::DeepSeekV4Chat,
        version: DEEPSEEK_V4_CHAT_PROFILE_VERSION,
    },
    ProviderProtocolDialect::OpenAiChatCompletions,
    ProviderRuntimeCapabilities::exact_provider_grouped(),
    ProviderAdapterKind::DeepSeekV4Chat,
    "深度求索 / DeepSeek（V4 Chat）",
    ProviderProfileSettingsKind::DeepseekV4Chat,
    true,
);

static PROVIDER_REGISTRATIONS: [&ProviderRegistration; 3] = [
    &GENERIC_OPENAI_CHAT_REGISTRATION,
    &GENERIC_ANTHROPIC_MESSAGES_REGISTRATION,
    &DEEPSEEK_V4_CHAT_REGISTRATION,
];

pub(crate) fn is_registered_provider_profile(profile: ProviderProfileRef) -> bool {
    PROVIDER_REGISTRATIONS
        .iter()
        .any(|registration| registration.profile == profile)
}

pub fn provider_profile_ui_descriptors() -> Vec<ProviderProfileUiDescriptor> {
    PROVIDER_REGISTRATIONS
        .iter()
        .map(|registration| registration.ui_descriptor())
        .collect()
}

pub fn resolve_ui_selectable_provider_registration(
    profile_id: ProviderProfileId,
    dialect: ProviderProtocolDialect,
) -> Result<&'static ProviderRegistration, ProviderProfileValidationError> {
    let Some(registration) = PROVIDER_REGISTRATIONS
        .iter()
        .copied()
        .find(|registration| registration.profile.id == profile_id && registration.ui_selectable)
    else {
        return Err(ProviderProfileValidationError::ProfileIsNotUiSelectable { id: profile_id });
    };
    if registration.dialect != dialect {
        return Err(ProviderProfileValidationError::IncompatibleDialect {
            id: profile_id,
            dialect,
        });
    }
    Ok(registration)
}

pub fn resolve_provider_registration(
    profile: ProviderProfileRef,
    dialect: ProviderProtocolDialect,
) -> Result<&'static ProviderRegistration, ProviderProfileValidationError> {
    if let Some(registration) = PROVIDER_REGISTRATIONS
        .iter()
        .copied()
        .find(|registration| registration.profile == profile && registration.dialect == dialect)
    {
        return Ok(registration);
    }
    if is_registered_provider_profile(profile) {
        return Err(ProviderProfileValidationError::IncompatibleDialect {
            id: profile.id,
            dialect,
        });
    }
    Err(ProviderProfileValidationError::UnsupportedProfileVersion {
        id: profile.id,
        version: profile.version,
    })
}

pub fn resolve_provider_registration_for_key(
    key: &ProviderProtocolKey,
) -> Result<&'static ProviderRegistration, ProviderProfileValidationError> {
    key.validate()?;
    resolve_provider_registration(key.profile, key.dialect)
}

pub fn resolve_provider_runtime_capabilities(
    key: &ProviderProtocolKey,
) -> Result<ProviderRuntimeCapabilities, ProviderProfileValidationError> {
    resolve_provider_registration_for_key(key).map(ProviderRegistration::runtime_capabilities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_profile::{ReasoningEffort, ReasoningPolicy};
    use serde_json::json;

    #[test]
    fn registry_resolves_exact_profile_version_and_dialect() {
        let generic_openai = resolve_provider_registration(
            ProviderProfileRef::generic_for_dialect(ProviderProtocolDialect::OpenAiChatCompletions),
            ProviderProtocolDialect::OpenAiChatCompletions,
        )
        .unwrap();
        assert_eq!(
            generic_openai.runtime_capabilities(),
            ProviderRuntimeCapabilities::generic()
        );

        let deepseek = resolve_provider_registration(
            ProviderProfileRef::deepseek_v4_chat(),
            ProviderProtocolDialect::OpenAiChatCompletions,
        )
        .unwrap();
        assert_eq!(
            deepseek.runtime_capabilities(),
            ProviderRuntimeCapabilities::exact_provider_grouped()
        );
    }

    #[test]
    fn renderer_projection_is_safe_and_registered_selection_is_host_versioned() {
        let descriptors = provider_profile_ui_descriptors();
        assert_eq!(descriptors.len(), 3);
        let wire = serde_json::to_value(&descriptors).unwrap();
        let deepseek = wire
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["profileId"] == "deepseek_v4_chat")
            .unwrap();
        assert_eq!(deepseek["profileVersion"], DEEPSEEK_V4_CHAT_PROFILE_VERSION);
        assert_eq!(deepseek["settingsKind"], "deepseek_v4_chat");
        assert_eq!(deepseek["selectable"], true);
        for private_field in [
            "runtimeCapabilities",
            "continuationRequirement",
            "toolExchange",
            "usage",
            "adapter",
        ] {
            assert!(deepseek.get(private_field).is_none());
        }

        let registration = resolve_ui_selectable_provider_registration(
            ProviderProfileId::parse("deepseek_v4_chat").unwrap(),
            ProviderProtocolDialect::OpenAiChatCompletions,
        )
        .unwrap();
        let normalized = registration
            .config_from_public_settings(ProviderProfilePublicSettings::DeepseekV4Chat {
                reasoning: ReasoningPolicy {
                    mode: ReasoningMode::Disabled,
                    effort: ReasoningEffort::Max,
                },
            })
            .unwrap();
        assert_eq!(normalized.profile, ProviderProfileRef::deepseek_v4_chat());
        assert_eq!(
            normalized.reasoning.effort,
            ReasoningEffort::ProviderDefault
        );

        assert_eq!(
            serde_json::to_value(normalized.reasoning).unwrap(),
            json!({"mode": "disabled", "effort": "provider_default"})
        );
    }

    #[test]
    fn generic_registrations_are_described_but_not_selectable_as_registered_profiles() {
        let result = resolve_ui_selectable_provider_registration(
            ProviderProfileId::GenericOpenAiChat,
            ProviderProtocolDialect::OpenAiChatCompletions,
        );
        assert!(matches!(
            result,
            Err(ProviderProfileValidationError::ProfileIsNotUiSelectable { .. })
        ));
    }

    #[test]
    fn registered_capability_contract_is_a_versioned_snapshot() {
        let generic = resolve_provider_registration(
            ProviderProfileRef {
                id: ProviderProfileId::GenericOpenAiChat,
                version: 1,
            },
            ProviderProtocolDialect::OpenAiChatCompletions,
        )
        .unwrap()
        .runtime_capabilities();
        assert_eq!(
            generic.tool_exchange(),
            ProviderToolExchangeSemantics::LegacySplit
        );
        assert_eq!(
            generic.private_replay(),
            ProviderPrivateReplaySemantics::None
        );
        assert_eq!(
            generic.context_projection(),
            ProviderContextProjectionSemantics::LegacyEffectiveCalls
        );
        assert_eq!(generic.usage(), ProviderUsageSemantics::StandardAdditive);
        assert_eq!(
            generic.partial_trace(),
            ProviderPartialTraceSemantics::IncrementalBaseline
        );
        assert_eq!(
            generic.tool_call_source(),
            ProviderToolCallSourceSemantics::LegacyTextFallbackAllowed
        );
        assert_eq!(
            generic.terminal_batch(),
            ProviderTerminalBatchSemantics::IndependentCalls
        );
        assert_eq!(
            generic.same_turn_skill_activation(),
            ProviderSameTurnSkillActivationSemantics::FilterUnactivatedSiblings
        );
        assert_eq!(
            generic.checkpoint_private_arguments(),
            ProviderCheckpointPrivateArgumentsSemantics::Reject
        );

        let deepseek = resolve_provider_registration(
            ProviderProfileRef {
                id: ProviderProfileId::DeepSeekV4Chat,
                version: 1,
            },
            ProviderProtocolDialect::OpenAiChatCompletions,
        )
        .unwrap()
        .runtime_capabilities();
        assert_eq!(
            deepseek.tool_exchange(),
            ProviderToolExchangeSemantics::ExactProviderGrouped
        );
        assert_eq!(
            deepseek.private_replay(),
            ProviderPrivateReplaySemantics::AdapterClassified
        );
        assert_eq!(
            deepseek.context_projection(),
            ProviderContextProjectionSemantics::ExactProviderTurn
        );
        assert_eq!(
            deepseek.usage(),
            ProviderUsageSemantics::CompletionIncludesReasoning
        );
        assert_eq!(
            deepseek.partial_trace(),
            ProviderPartialTraceSemantics::DeferUntilProviderTurnClosed
        );
        assert_eq!(
            deepseek.tool_call_source(),
            ProviderToolCallSourceSemantics::ProviderNativeOnly
        );
        assert_eq!(
            deepseek.terminal_batch(),
            ProviderTerminalBatchSemantics::CloseWholeProviderTurn
        );
        assert_eq!(
            deepseek.same_turn_skill_activation(),
            ProviderSameTurnSkillActivationSemantics::PreserveAndGuardSiblings
        );
        assert_eq!(
            deepseek.checkpoint_private_arguments(),
            ProviderCheckpointPrivateArgumentsSemantics::RehydrateFromAuthenticatedTurn
        );
    }

    #[test]
    fn lifecycle_axes_do_not_inherit_tool_exchange_or_private_replay() {
        let independently_enabled = ProviderRuntimeCapabilities {
            tool_exchange: ProviderToolExchangeSemantics::LegacySplit,
            private_replay: ProviderPrivateReplaySemantics::None,
            context_projection: ProviderContextProjectionSemantics::LegacyEffectiveCalls,
            usage: ProviderUsageSemantics::StandardAdditive,
            partial_trace: ProviderPartialTraceSemantics::IncrementalBaseline,
            tool_call_source: ProviderToolCallSourceSemantics::ProviderNativeOnly,
            terminal_batch: ProviderTerminalBatchSemantics::CloseWholeProviderTurn,
            same_turn_skill_activation:
                ProviderSameTurnSkillActivationSemantics::PreserveAndGuardSiblings,
            checkpoint_private_arguments:
                ProviderCheckpointPrivateArgumentsSemantics::RehydrateFromAuthenticatedTurn,
        };
        assert!(independently_enabled.requires_provider_native_tool_calls());
        assert!(independently_enabled.preserves_skill_activation_batch());
        assert!(independently_enabled.allows_encrypted_checkpoint_rehydration());
        let enabled_policy =
            independently_enabled.classify_turn(true, false, ReasoningMode::ProviderDefault);
        assert!(!enabled_policy.preserves_grouped_turn());
        assert!(!enabled_policy.requires_private_replay());
        assert!(enabled_policy.allows_encrypted_checkpoint_rehydration());
        assert!(enabled_policy.settles_entire_batch_on_terminal());

        let independently_disabled = ProviderRuntimeCapabilities {
            tool_exchange: ProviderToolExchangeSemantics::ExactProviderGrouped,
            private_replay: ProviderPrivateReplaySemantics::AdapterClassified,
            context_projection: ProviderContextProjectionSemantics::ExactProviderTurn,
            usage: ProviderUsageSemantics::CompletionIncludesReasoning,
            partial_trace: ProviderPartialTraceSemantics::DeferUntilProviderTurnClosed,
            tool_call_source: ProviderToolCallSourceSemantics::LegacyTextFallbackAllowed,
            terminal_batch: ProviderTerminalBatchSemantics::IndependentCalls,
            same_turn_skill_activation:
                ProviderSameTurnSkillActivationSemantics::FilterUnactivatedSiblings,
            checkpoint_private_arguments: ProviderCheckpointPrivateArgumentsSemantics::Reject,
        };
        assert!(!independently_disabled.requires_provider_native_tool_calls());
        assert!(!independently_disabled.preserves_skill_activation_batch());
        assert!(!independently_disabled.allows_encrypted_checkpoint_rehydration());
        let disabled_policy =
            independently_disabled.classify_turn(true, false, ReasoningMode::ProviderDefault);
        assert!(disabled_policy.preserves_grouped_turn());
        assert!(disabled_policy.requires_private_replay());
        assert!(disabled_policy.requires_exact_approval_refs());
        assert!(!disabled_policy.allows_encrypted_checkpoint_rehydration());
        assert!(!disabled_policy.settles_entire_batch_on_terminal());
    }

    #[test]
    fn registry_fails_closed_for_unknown_version_and_wrong_dialect() {
        let unknown_version = ProviderProfileRef {
            id: ProviderProfileId::DeepSeekV4Chat,
            version: DEEPSEEK_V4_CHAT_PROFILE_VERSION + 1,
        };
        assert!(matches!(
            resolve_provider_registration(
                unknown_version,
                ProviderProtocolDialect::OpenAiChatCompletions
            ),
            Err(ProviderProfileValidationError::UnsupportedProfileVersion { .. })
        ));
        assert!(matches!(
            resolve_provider_registration(
                ProviderProfileRef::deepseek_v4_chat(),
                ProviderProtocolDialect::AnthropicMessages
            ),
            Err(ProviderProfileValidationError::IncompatibleDialect { .. })
        ));
    }

    #[test]
    fn turn_policy_is_derived_without_provider_identity_checks() {
        let generic = ProviderRuntimeCapabilities::generic().classify_turn(
            true,
            false,
            ReasoningMode::ProviderDefault,
        );
        assert!(!generic.requires_private_replay());
        assert_eq!(
            generic.continuation_requirement(),
            ProviderContinuationRequirement::Forbidden
        );
        assert!(!generic.preserves_grouped_turn());

        let deepseek_enabled = ProviderRuntimeCapabilities::exact_provider_grouped().classify_turn(
            true,
            false,
            ReasoningMode::Enabled,
        );
        assert!(deepseek_enabled.requires_private_replay());
        assert_eq!(
            deepseek_enabled.continuation_requirement(),
            ProviderContinuationRequirement::Required
        );
        assert!(deepseek_enabled.preserves_grouped_turn());
        assert!(deepseek_enabled.requires_exact_approval_refs());
        assert!(deepseek_enabled.allows_encrypted_checkpoint_rehydration());
        assert!(deepseek_enabled.settles_entire_batch_on_terminal());

        let deepseek_default = ProviderRuntimeCapabilities::exact_provider_grouped().classify_turn(
            true,
            false,
            ReasoningMode::ProviderDefault,
        );
        assert_eq!(
            deepseek_default.continuation_requirement(),
            ProviderContinuationRequirement::Optional
        );
    }
}
