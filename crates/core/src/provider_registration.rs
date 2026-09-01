//! Code-owned Provider Profile registrations and runtime behavior contracts.
//!
//! These values are deliberately not serializable. Persisted settings select a versioned
//! [`ProviderProfileRef`]; the running binary resolves that exact identity and dialect to one
//! immutable registration. Renderer or stored JSON therefore cannot manufacture runtime powers.

use crate::provider_profile::{
    MoonshotK26ThinkingMode, ProviderFamilySettings, ProviderModelFamilyId, ProviderProfileConfig,
    ProviderProfileConfigV1, ProviderProfileId, ProviderProfilePublicSettings, ProviderProfileRef,
    ProviderProfileValidationError, ProviderProtocolDialect, ProviderProtocolKey,
    ProviderReasoningEffort, ProviderVendorId, ProviderVendorPublicSettings, ReasoningMode,
    PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};

mod deepseek;
mod generic;
mod moonshot;

pub(crate) use deepseek::{DEEPSEEK_V4_CHAT_REGISTRATION, DEEPSEEK_V4_VISION_REGISTRATION};
pub(crate) use generic::{
    GENERIC_ANTHROPIC_MESSAGES_REGISTRATION, GENERIC_OPENAI_CHAT_REGISTRATION,
};
pub(crate) use moonshot::{
    MOONSHOT_K2_6_CHAT_REGISTRATION, MOONSHOT_K2_7_CODE_CHAT_REGISTRATION,
    MOONSHOT_K3_CHAT_REGISTRATION,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderToolExchangeSemantics {
    SplitToolExchange,
    ExactProviderGrouped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderPrivateReplaySemantics {
    None,
    AdapterClassified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderContextProjectionSemantics {
    GenericEffectiveCalls,
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
    TextFallbackAllowed,
    ProviderNativeOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderTerminalBatchSemantics {
    IndependentCalls,
    CloseWholeProviderTurn,
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

/// Stable, vendor-level option projected to Renderer without an internal Profile identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderVendorDescriptor {
    pub vendor_id: ProviderVendorId,
    pub display_name: &'static str,
    pub selectable: bool,
}

/// Public editor family. This contains no runtime, replay, checkpoint or usage capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderVendorSettingsKind {
    None,
    Deepseek,
    Moonshot,
}

/// Safe, bounded request used only to preview the Host-authoritative vendor/model resolution.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderVendorModelPolicyInput {
    pub vendor_id: ProviderVendorId,
    pub model_id: String,
    pub dialect: ProviderProtocolDialect,
}

/// Legal public settings for the family selected by the Host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProviderFamilySettingsDescriptor {
    Generic {
        default_settings: ProviderFamilySettings,
    },
    DeepseekV4Chat {
        reasoning_modes: Vec<ReasoningMode>,
        reasoning_efforts: Vec<ProviderReasoningEffort>,
        default_settings: ProviderFamilySettings,
    },
    DeepseekV4Vision {
        reasoning_modes: Vec<ReasoningMode>,
        reasoning_efforts: Vec<ProviderReasoningEffort>,
        default_settings: ProviderFamilySettings,
    },
    MoonshotK3Chat {
        reasoning_efforts: Vec<ProviderReasoningEffort>,
        default_settings: ProviderFamilySettings,
    },
    #[serde(rename = "moonshot_k2_7_code_chat")]
    MoonshotK27CodeChat {
        default_settings: ProviderFamilySettings,
    },
    #[serde(rename = "moonshot_k2_6_chat")]
    MoonshotK26Chat {
        thinking_modes: Vec<MoonshotK26ThinkingMode>,
        default_settings: ProviderFamilySettings,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderVendorModelUnsupportedReason {
    UnsupportedVendor,
    UnsupportedDialect,
    UnsupportedModel,
}

/// Sanitized result of vendor + exact model id + dialect resolution.
///
/// Internal Profile ids/versions and all runtime capabilities are deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProviderVendorModelPolicyDescriptor {
    Supported {
        vendor_id: ProviderVendorId,
        model_family: ProviderModelFamilyId,
        settings_kind: ProviderVendorSettingsKind,
        settings: ProviderFamilySettingsDescriptor,
    },
    Unsupported {
        vendor_id: ProviderVendorId,
        reason: ProviderVendorModelUnsupportedReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderVendorResolutionError {
    UnsupportedVendor { vendor_id: ProviderVendorId },
    UnsupportedDialect { vendor_id: ProviderVendorId },
    UnsupportedModel { vendor_id: ProviderVendorId },
}

impl std::fmt::Display for ProviderVendorResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVendor { vendor_id } => {
                write!(formatter, "provider vendor {vendor_id} is not registered")
            }
            Self::UnsupportedDialect { vendor_id } => {
                write!(
                    formatter,
                    "provider vendor {vendor_id} does not support this dialect"
                )
            }
            Self::UnsupportedModel { vendor_id } => {
                write!(
                    formatter,
                    "provider vendor {vendor_id} does not support this model id"
                )
            }
        }
    }
}

impl std::error::Error for ProviderVendorResolutionError {}

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
    checkpoint_private_arguments: ProviderCheckpointPrivateArgumentsSemantics,
}

impl ProviderRuntimeCapabilities {
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

    pub const fn checkpoint_private_arguments(self) -> ProviderCheckpointPrivateArgumentsSemantics {
        self.checkpoint_private_arguments
    }

    pub const fn requires_provider_native_tool_calls(self) -> bool {
        matches!(
            self.tool_call_source,
            ProviderToolCallSourceSemantics::ProviderNativeOnly
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
        } else if has_provider_tool_calls && !matches!(reasoning_mode, ReasoningMode::Disabled) {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderModelIdPolicy {
    AnyNonEmpty,
    Exact(&'static [&'static str]),
}

impl ProviderModelIdPolicy {
    fn accepts(self, model_id: &str) -> bool {
        !model_id.is_empty()
            && model_id.len() <= 2 * 1024
            && model_id.trim() == model_id
            && match self {
                Self::AnyNonEmpty => true,
                Self::Exact(ids) => ids.contains(&model_id),
            }
    }
}

type ProviderSettingsDescriptorFactory = fn() -> ProviderFamilySettingsDescriptor;
type ProviderSettingsValidator = fn(ProviderFamilySettings) -> bool;

#[derive(Debug)]
pub struct ProviderRegistration {
    profile: ProviderProfileRef,
    vendor_id: ProviderVendorId,
    family_id: ProviderModelFamilyId,
    dialect: ProviderProtocolDialect,
    model_id_policy: ProviderModelIdPolicy,
    runtime_capabilities: ProviderRuntimeCapabilities,
    adapter_kind: ProviderAdapterKind,
    display_name: &'static str,
    settings_kind: ProviderProfileSettingsKind,
    vendor_settings_kind: ProviderVendorSettingsKind,
    settings_descriptor: ProviderSettingsDescriptorFactory,
    settings_validator: ProviderSettingsValidator,
    accepts_legacy_v1: bool,
    ui_selectable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ProviderAdapterKind {
    GenericOpenAi,
    GenericAnthropic,
    DeepSeekV4Chat,
    DeepSeekV4Vision,
    MoonshotK3Chat,
    MoonshotK27CodeChat,
    MoonshotK26Chat,
}

impl ProviderRegistration {
    #[allow(clippy::too_many_arguments)]
    const fn new(
        profile: ProviderProfileRef,
        vendor_id: ProviderVendorId,
        family_id: ProviderModelFamilyId,
        dialect: ProviderProtocolDialect,
        model_id_policy: ProviderModelIdPolicy,
        runtime_capabilities: ProviderRuntimeCapabilities,
        adapter_kind: ProviderAdapterKind,
        display_name: &'static str,
        settings_kind: ProviderProfileSettingsKind,
        vendor_settings_kind: ProviderVendorSettingsKind,
        settings_descriptor: ProviderSettingsDescriptorFactory,
        settings_validator: ProviderSettingsValidator,
        accepts_legacy_v1: bool,
        ui_selectable: bool,
    ) -> Self {
        Self {
            profile,
            vendor_id,
            family_id,
            dialect,
            model_id_policy,
            runtime_capabilities,
            adapter_kind,
            display_name,
            settings_kind,
            vendor_settings_kind,
            settings_descriptor,
            settings_validator,
            accepts_legacy_v1,
            ui_selectable,
        }
    }

    pub const fn profile(&self) -> ProviderProfileRef {
        self.profile
    }

    pub const fn dialect(&self) -> ProviderProtocolDialect {
        self.dialect
    }

    pub const fn vendor_id(&self) -> ProviderVendorId {
        self.vendor_id
    }

    pub const fn family_id(&self) -> ProviderModelFamilyId {
        self.family_id
    }

    pub const fn runtime_capabilities(&self) -> ProviderRuntimeCapabilities {
        self.runtime_capabilities
    }

    pub(crate) const fn adapter_kind(&self) -> ProviderAdapterKind {
        self.adapter_kind
    }

    pub fn vendor_settings_descriptor(&self) -> ProviderFamilySettingsDescriptor {
        (self.settings_descriptor)()
    }

    pub const fn vendor_settings_kind(&self) -> ProviderVendorSettingsKind {
        self.vendor_settings_kind
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
        let config = ProviderProfileConfig::V1(ProviderProfileConfigV1 {
            schema_version: PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
            profile: self.profile,
            reasoning: settings.reasoning(),
        });
        config.validate_for_dialect(self.dialect)?;
        Ok(config)
    }

    pub fn config_from_vendor_settings(
        &self,
        settings: ProviderVendorPublicSettings,
    ) -> Result<ProviderProfileConfig, ProviderProfileValidationError> {
        let settings = settings.normalized();
        if !(self.settings_validator)(settings) {
            return Err(ProviderProfileValidationError::FamilySettingsKindMismatch {
                id: self.profile.id,
            });
        }
        let config =
            ProviderProfileConfig::from_family_settings(self.profile, self.vendor_id, settings);
        config.validate_for_dialect(self.dialect)?;
        Ok(config)
    }

    pub(crate) fn validate_profile_config(
        &self,
        config: &ProviderProfileConfig,
    ) -> Result<(), ProviderProfileValidationError> {
        if config.profile() != self.profile {
            return Err(ProviderProfileValidationError::ProfileKeyMismatch);
        }
        match config {
            ProviderProfileConfig::V1(_) if self.accepts_legacy_v1 => Ok(()),
            ProviderProfileConfig::V1(_) => {
                Err(ProviderProfileValidationError::FamilySettingsKindMismatch {
                    id: self.profile.id,
                })
            }
            ProviderProfileConfig::V2(config) => {
                if config.vendor_id != self.vendor_id {
                    return Err(ProviderProfileValidationError::VendorMismatch {
                        id: self.profile.id,
                        vendor_id: config.vendor_id,
                    });
                }
                if !(self.settings_validator)(config.settings) {
                    return Err(ProviderProfileValidationError::FamilySettingsKindMismatch {
                        id: self.profile.id,
                    });
                }
                Ok(())
            }
        }
    }

    pub(crate) fn validate_model_id(
        &self,
        model_id: &str,
    ) -> Result<(), ProviderProfileValidationError> {
        if self.model_id_policy.accepts(model_id) {
            Ok(())
        } else {
            Err(ProviderProfileValidationError::UnsupportedModelForProfile {
                id: self.profile.id,
            })
        }
    }
}

static PROVIDER_REGISTRATIONS: [&ProviderRegistration; 7] = [
    &GENERIC_OPENAI_CHAT_REGISTRATION,
    &GENERIC_ANTHROPIC_MESSAGES_REGISTRATION,
    &DEEPSEEK_V4_CHAT_REGISTRATION,
    &DEEPSEEK_V4_VISION_REGISTRATION,
    &MOONSHOT_K3_CHAT_REGISTRATION,
    &MOONSHOT_K2_7_CODE_CHAT_REGISTRATION,
    &MOONSHOT_K2_6_CHAT_REGISTRATION,
];

// Compatibility surface consumed by the current Renderer. Keep contents, order, labels and
// profile-level settings kinds frozen until the dedicated vendor UI ships in round two.
static LEGACY_PROVIDER_UI_REGISTRATIONS: [&ProviderRegistration; 3] = [
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
    LEGACY_PROVIDER_UI_REGISTRATIONS
        .iter()
        .map(|registration| registration.ui_descriptor())
        .collect()
}

pub fn provider_vendor_descriptors() -> Vec<ProviderVendorDescriptor> {
    vec![
        generic::vendor_descriptor(),
        deepseek::vendor_descriptor(),
        moonshot::vendor_descriptor(),
    ]
}

pub fn resolve_provider_vendor_registration(
    vendor_id: ProviderVendorId,
    model_id: &str,
    dialect: ProviderProtocolDialect,
) -> Result<&'static ProviderRegistration, ProviderVendorResolutionError> {
    let vendor_registrations = PROVIDER_REGISTRATIONS
        .iter()
        .copied()
        .filter(|registration| registration.vendor_id == vendor_id)
        .collect::<Vec<_>>();
    if vendor_registrations.is_empty() {
        return Err(ProviderVendorResolutionError::UnsupportedVendor { vendor_id });
    }
    let dialect_registrations = vendor_registrations
        .into_iter()
        .filter(|registration| registration.dialect == dialect)
        .collect::<Vec<_>>();
    if dialect_registrations.is_empty() {
        return Err(ProviderVendorResolutionError::UnsupportedDialect { vendor_id });
    }
    dialect_registrations
        .into_iter()
        .find(|registration| registration.model_id_policy.accepts(model_id))
        .ok_or(ProviderVendorResolutionError::UnsupportedModel { vendor_id })
}

pub fn resolve_provider_vendor_model_policy(
    input: &ProviderVendorModelPolicyInput,
) -> ProviderVendorModelPolicyDescriptor {
    match resolve_provider_vendor_registration(input.vendor_id, &input.model_id, input.dialect) {
        Ok(registration) => ProviderVendorModelPolicyDescriptor::Supported {
            vendor_id: registration.vendor_id(),
            model_family: registration.family_id(),
            settings_kind: registration.vendor_settings_kind(),
            settings: registration.vendor_settings_descriptor(),
        },
        Err(error) => ProviderVendorModelPolicyDescriptor::Unsupported {
            vendor_id: input.vendor_id,
            reason: match error {
                ProviderVendorResolutionError::UnsupportedVendor { .. } => {
                    ProviderVendorModelUnsupportedReason::UnsupportedVendor
                }
                ProviderVendorResolutionError::UnsupportedDialect { .. } => {
                    ProviderVendorModelUnsupportedReason::UnsupportedDialect
                }
                ProviderVendorResolutionError::UnsupportedModel { .. } => {
                    ProviderVendorModelUnsupportedReason::UnsupportedModel
                }
            },
        },
    }
}

pub fn resolve_ui_selectable_provider_registration(
    profile_id: ProviderProfileId,
    dialect: ProviderProtocolDialect,
) -> Result<&'static ProviderRegistration, ProviderProfileValidationError> {
    let Some(registration) = LEGACY_PROVIDER_UI_REGISTRATIONS
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
    use crate::provider_profile::{
        ProviderFamilyReasoningPolicy, ReasoningEffort, ReasoningPolicy,
        DEEPSEEK_V4_CHAT_PROFILE_VERSION,
    };
    use serde_json::json;

    #[test]
    fn registry_resolves_exact_profile_version_and_dialect() {
        let generic_openai = resolve_provider_registration(
            ProviderProfileRef::generic_for_dialect(ProviderProtocolDialect::OpenAiChatCompletions),
            ProviderProtocolDialect::OpenAiChatCompletions,
        )
        .unwrap();
        assert_eq!(
            generic_openai.runtime_capabilities().tool_exchange(),
            ProviderToolExchangeSemantics::SplitToolExchange
        );

        let deepseek = resolve_provider_registration(
            ProviderProfileRef::deepseek_v4_chat(),
            ProviderProtocolDialect::OpenAiChatCompletions,
        )
        .unwrap();
        assert_eq!(
            deepseek.runtime_capabilities().tool_exchange(),
            ProviderToolExchangeSemantics::ExactProviderGrouped
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
        assert_eq!(normalized.profile(), ProviderProfileRef::deepseek_v4_chat());
        assert_eq!(
            normalized.legacy_reasoning().unwrap().effort,
            ReasoningEffort::ProviderDefault,
        );

        assert_eq!(
            serde_json::to_value(normalized.legacy_reasoning().unwrap()).unwrap(),
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
    fn vendor_projection_hides_profiles_and_resolves_exact_model_families() {
        let vendors = serde_json::to_value(provider_vendor_descriptors()).unwrap();
        assert_eq!(
            vendors,
            json!([
                {"vendorId": "generic", "displayName": "通用兼容", "selectable": true},
                {"vendorId": "deepseek", "displayName": "DeepSeek", "selectable": true},
                {"vendorId": "moonshot", "displayName": "月之暗面", "selectable": false}
            ])
        );
        let encoded = vendors.to_string();
        assert!(!encoded.contains("profileId"));
        assert!(!encoded.contains("runtimeCapabilities"));

        let policy = resolve_provider_vendor_model_policy(&ProviderVendorModelPolicyInput {
            vendor_id: ProviderVendorId::Moonshot,
            model_id: "kimi-k3".to_string(),
            dialect: ProviderProtocolDialect::OpenAiChatCompletions,
        });
        let wire = serde_json::to_value(policy).unwrap();
        assert_eq!(wire["status"], "supported");
        assert_eq!(wire["vendorId"], "moonshot");
        assert_eq!(wire["modelFamily"], "moonshot_k3_chat");
        assert_eq!(wire["settingsKind"], "moonshot");
        assert_eq!(
            wire["settings"]["defaultSettings"]["reasoningEffort"],
            "max"
        );
        assert!(wire.get("profileId").is_none());
        assert!(wire.get("profileVersion").is_none());
    }

    #[test]
    fn vendor_resolution_fails_closed_but_legacy_deepseek_profile_remains_compatible() {
        assert!(matches!(
            resolve_provider_vendor_registration(
                ProviderVendorId::Moonshot,
                "future-kimi-model",
                ProviderProtocolDialect::OpenAiChatCompletions,
            ),
            Err(ProviderVendorResolutionError::UnsupportedModel { .. })
        ));
        assert!(matches!(
            resolve_provider_vendor_registration(
                ProviderVendorId::Moonshot,
                "kimi-k3",
                ProviderProtocolDialect::AnthropicMessages,
            ),
            Err(ProviderVendorResolutionError::UnsupportedDialect { .. })
        ));

        let legacy = ProviderProfileConfig::deepseek_v4_default();
        assert!(ProviderProtocolKey::new(
            ProviderProtocolDialect::OpenAiChatCompletions,
            &legacy,
            "kimi-k3",
            None,
        )
        .is_ok());
    }

    #[test]
    fn vendor_selection_writes_v2_and_revalidates_family_and_exact_model() {
        let registration = resolve_provider_vendor_registration(
            ProviderVendorId::Moonshot,
            "kimi-k3",
            ProviderProtocolDialect::OpenAiChatCompletions,
        )
        .unwrap();
        let config = registration
            .config_from_vendor_settings(ProviderFamilySettings::MoonshotK3Chat {
                reasoning_effort: ProviderReasoningEffort::Low,
            })
            .unwrap();
        assert_eq!(config.schema_version(), 2);
        assert_eq!(config.vendor_id(), Some(ProviderVendorId::Moonshot));
        config
            .validate_for_model("kimi-k3", ProviderProtocolDialect::OpenAiChatCompletions)
            .unwrap();
        assert!(matches!(
            config.validate_for_model("kimi-k2.6", ProviderProtocolDialect::OpenAiChatCompletions,),
            Err(ProviderProfileValidationError::UnsupportedModelForProfile { .. })
        ));
        assert!(registration
            .config_from_vendor_settings(ProviderFamilySettings::DeepseekV4Chat {
                reasoning: ProviderFamilyReasoningPolicy::provider_default(),
            })
            .is_err());
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
            ProviderToolExchangeSemantics::SplitToolExchange
        );
        assert_eq!(
            generic.private_replay(),
            ProviderPrivateReplaySemantics::None
        );
        assert_eq!(
            generic.context_projection(),
            ProviderContextProjectionSemantics::GenericEffectiveCalls
        );
        assert_eq!(generic.usage(), ProviderUsageSemantics::StandardAdditive);
        assert_eq!(
            generic.partial_trace(),
            ProviderPartialTraceSemantics::IncrementalBaseline
        );
        assert_eq!(
            generic.tool_call_source(),
            ProviderToolCallSourceSemantics::TextFallbackAllowed
        );
        assert_eq!(
            generic.terminal_batch(),
            ProviderTerminalBatchSemantics::IndependentCalls
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
            deepseek.checkpoint_private_arguments(),
            ProviderCheckpointPrivateArgumentsSemantics::RehydrateFromAuthenticatedTurn
        );
    }

    #[test]
    fn lifecycle_axes_do_not_inherit_tool_exchange_or_private_replay() {
        let independently_enabled = ProviderRuntimeCapabilities {
            tool_exchange: ProviderToolExchangeSemantics::SplitToolExchange,
            private_replay: ProviderPrivateReplaySemantics::None,
            context_projection: ProviderContextProjectionSemantics::GenericEffectiveCalls,
            usage: ProviderUsageSemantics::StandardAdditive,
            partial_trace: ProviderPartialTraceSemantics::IncrementalBaseline,
            tool_call_source: ProviderToolCallSourceSemantics::ProviderNativeOnly,
            terminal_batch: ProviderTerminalBatchSemantics::CloseWholeProviderTurn,
            checkpoint_private_arguments:
                ProviderCheckpointPrivateArgumentsSemantics::RehydrateFromAuthenticatedTurn,
        };
        assert!(independently_enabled.requires_provider_native_tool_calls());
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
            tool_call_source: ProviderToolCallSourceSemantics::TextFallbackAllowed,
            terminal_batch: ProviderTerminalBatchSemantics::IndependentCalls,
            checkpoint_private_arguments: ProviderCheckpointPrivateArgumentsSemantics::Reject,
        };
        assert!(!independently_disabled.requires_provider_native_tool_calls());
        assert!(!independently_disabled.allows_encrypted_checkpoint_rehydration());
        let disabled_policy =
            independently_disabled.classify_turn(true, false, ReasoningMode::ProviderDefault);
        assert!(disabled_policy.preserves_grouped_turn());
        assert!(disabled_policy.requires_private_replay());
        assert_eq!(
            disabled_policy.continuation_requirement(),
            ProviderContinuationRequirement::Required
        );
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
        let generic = GENERIC_OPENAI_CHAT_REGISTRATION
            .runtime_capabilities()
            .classify_turn(true, false, ReasoningMode::ProviderDefault);
        assert!(!generic.requires_private_replay());
        assert_eq!(
            generic.continuation_requirement(),
            ProviderContinuationRequirement::Forbidden
        );
        assert!(!generic.preserves_grouped_turn());

        let deepseek_capabilities = DEEPSEEK_V4_CHAT_REGISTRATION.runtime_capabilities();
        let deepseek_enabled =
            deepseek_capabilities.classify_turn(true, false, ReasoningMode::Enabled);
        assert!(deepseek_enabled.requires_private_replay());
        assert_eq!(
            deepseek_enabled.continuation_requirement(),
            ProviderContinuationRequirement::Required
        );
        assert!(deepseek_enabled.preserves_grouped_turn());
        assert!(deepseek_enabled.requires_exact_approval_refs());
        assert!(deepseek_enabled.allows_encrypted_checkpoint_rehydration());
        assert!(deepseek_enabled.settles_entire_batch_on_terminal());

        let deepseek_default =
            deepseek_capabilities.classify_turn(true, false, ReasoningMode::ProviderDefault);
        assert_eq!(
            deepseek_default.continuation_requirement(),
            ProviderContinuationRequirement::Required
        );
    }
}
