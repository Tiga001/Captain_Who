use mycopilot_core::protocol::{
    AgentBuiltinExecutionPermission, AgentCommandPermission, AgentCommandSafetyPolicy,
    AgentPatchPermission, AgentPermissions, AgentReadPermission, AgentWritePermission,
};
use mycopilot_core::provider_profile::{ProviderReasoningEffort, ReasoningMode};
use mycopilot_core::storage::models::{ModelConfigRecord, UiPreferencesRecord};
use mycopilot_protocol_rs::{
    AutomationBuiltinExecutionPermissionDto, AutomationCommandPermissionDto,
    AutomationCommandSafetyPolicyDto, AutomationPatchPermissionDto, AutomationPermissionModeDto,
    AutomationReadPermissionDto, AutomationReasoningEffortDto, AutomationReasoningModeDto,
    AutomationReasoningProjectionDto, AutomationReasoningSourceDto,
    AutomationResolvedPermissionsDto, AutomationWritePermissionDto,
    AUTOMATION_PERMISSION_MODE_VERSION,
};
use std::error::Error;
use std::fmt::{Display, Formatter};

/// Resolved Host authority frozen onto an automation task.
///
/// The Agent-domain value is used for execution; the DTO is the exact safe projection persisted
/// and returned to Renderer. Keeping both in one result makes drift at service call sites harder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedAutomationPermissions {
    pub(crate) mode: AutomationPermissionModeDto,
    pub(crate) mode_version: u32,
    pub(crate) permissions: AgentPermissions,
    pub(crate) projection: AutomationResolvedPermissionsDto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomationPermissionResolveError {
    UnsupportedModeVersion { requested: u32, supported: u32 },
    FullPermissionDisabled,
    CustomPermissionDisabled,
}

impl AutomationPermissionResolveError {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::UnsupportedModeVersion { .. } => "unsupported_permission_mode_version",
            Self::FullPermissionDisabled | Self::CustomPermissionDisabled => "permission_disabled",
        }
    }
}

impl Display for AutomationPermissionResolveError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedModeVersion {
                requested,
                supported,
            } => write!(
                formatter,
                "unsupported automation permission mode version {requested}; expected {supported}"
            ),
            Self::FullPermissionDisabled => {
                formatter.write_str("full permission mode is disabled in user settings")
            }
            Self::CustomPermissionDisabled => {
                formatter.write_str("custom permission mode is disabled in user settings")
            }
        }
    }
}

impl Error for AutomationPermissionResolveError {}

/// Resolves the same three permission modes as `chatPermissions.ts` without ever invoking
/// `AgentPermissions::default()` (whose denied-write default intentionally has different meaning).
pub(crate) fn resolve_automation_permissions(
    mode: AutomationPermissionModeDto,
    mode_version: u32,
    preferences: &UiPreferencesRecord,
) -> Result<ResolvedAutomationPermissions, AutomationPermissionResolveError> {
    if mode_version != AUTOMATION_PERMISSION_MODE_VERSION {
        return Err(AutomationPermissionResolveError::UnsupportedModeVersion {
            requested: mode_version,
            supported: AUTOMATION_PERMISSION_MODE_VERSION,
        });
    }

    let permissions = match mode {
        AutomationPermissionModeDto::Default => default_chat_permissions(),
        AutomationPermissionModeDto::Full => {
            if !preferences.full_permission_enabled {
                return Err(AutomationPermissionResolveError::FullPermissionDisabled);
            }
            full_chat_permissions()
        }
        AutomationPermissionModeDto::Custom => {
            if !preferences.custom_permission_enabled {
                return Err(AutomationPermissionResolveError::CustomPermissionDisabled);
            }
            // Composer custom mode is always guarded even if a corrupt or future settings value
            // were to carry another command-safety policy.
            AgentPermissions {
                command_safety: AgentCommandSafetyPolicy::Guarded,
                ..preferences.custom_permissions
            }
        }
    };

    Ok(ResolvedAutomationPermissions {
        mode,
        mode_version: AUTOMATION_PERMISSION_MODE_VERSION,
        permissions,
        projection: permissions_projection(permissions),
    })
}

/// Re-checks the user's current enablement as a revocation ceiling before every future run.
/// The caller should continue executing the task's stored snapshot when this succeeds; it must
/// never silently resolve a new, potentially broader custom snapshot here.
pub(crate) fn ensure_automation_permission_mode_enabled(
    mode: AutomationPermissionModeDto,
    preferences: &UiPreferencesRecord,
) -> Result<(), AutomationPermissionResolveError> {
    match mode {
        AutomationPermissionModeDto::Default => Ok(()),
        AutomationPermissionModeDto::Full if preferences.full_permission_enabled => Ok(()),
        AutomationPermissionModeDto::Full => {
            Err(AutomationPermissionResolveError::FullPermissionDisabled)
        }
        AutomationPermissionModeDto::Custom if preferences.custom_permission_enabled => Ok(()),
        AutomationPermissionModeDto::Custom => {
            Err(AutomationPermissionResolveError::CustomPermissionDisabled)
        }
    }
}

pub(crate) fn permissions_projection(
    permissions: AgentPermissions,
) -> AutomationResolvedPermissionsDto {
    AutomationResolvedPermissionsDto {
        read: match permissions.read {
            AgentReadPermission::WorkspaceOnly => AutomationReadPermissionDto::WorkspaceOnly,
            AgentReadPermission::All => AutomationReadPermissionDto::All,
        },
        write: match permissions.write {
            AgentWritePermission::Denied => AutomationWritePermissionDto::Denied,
            AgentWritePermission::WorkspaceOnly => AutomationWritePermissionDto::WorkspaceOnly,
            AgentWritePermission::All => AutomationWritePermissionDto::All,
        },
        command: match permissions.command {
            AgentCommandPermission::RequireApproval => {
                AutomationCommandPermissionDto::RequireApproval
            }
            AgentCommandPermission::AutoApprove => AutomationCommandPermissionDto::AutoApprove,
        },
        command_safety: match permissions.command_safety {
            AgentCommandSafetyPolicy::Guarded => AutomationCommandSafetyPolicyDto::Guarded,
            AgentCommandSafetyPolicy::FullAccess => AutomationCommandSafetyPolicyDto::FullAccess,
        },
        patch: match permissions.patch {
            AgentPatchPermission::RequireApproval => AutomationPatchPermissionDto::RequireApproval,
            AgentPatchPermission::AutoApprove => AutomationPatchPermissionDto::AutoApprove,
        },
        builtin_execution: match permissions.builtin_execution {
            AgentBuiltinExecutionPermission::RequireApproval => {
                AutomationBuiltinExecutionPermissionDto::RequireApproval
            }
            AgentBuiltinExecutionPermission::AutoApprove => {
                AutomationBuiltinExecutionPermissionDto::AutoApprove
            }
        },
    }
}

/// Restores the exact Agent authority frozen in one automation configuration snapshot.
/// Current UI preferences are only a revocation ceiling and never broaden or re-resolve this
/// value at execution time.
pub(crate) fn permissions_from_projection(
    projection: AutomationResolvedPermissionsDto,
) -> AgentPermissions {
    AgentPermissions {
        read: match projection.read {
            AutomationReadPermissionDto::WorkspaceOnly => AgentReadPermission::WorkspaceOnly,
            AutomationReadPermissionDto::All => AgentReadPermission::All,
        },
        write: match projection.write {
            AutomationWritePermissionDto::Denied => AgentWritePermission::Denied,
            AutomationWritePermissionDto::WorkspaceOnly => AgentWritePermission::WorkspaceOnly,
            AutomationWritePermissionDto::All => AgentWritePermission::All,
        },
        command: match projection.command {
            AutomationCommandPermissionDto::RequireApproval => {
                AgentCommandPermission::RequireApproval
            }
            AutomationCommandPermissionDto::AutoApprove => AgentCommandPermission::AutoApprove,
        },
        command_safety: match projection.command_safety {
            AutomationCommandSafetyPolicyDto::Guarded => AgentCommandSafetyPolicy::Guarded,
            AutomationCommandSafetyPolicyDto::FullAccess => AgentCommandSafetyPolicy::FullAccess,
        },
        patch: match projection.patch {
            AutomationPatchPermissionDto::RequireApproval => AgentPatchPermission::RequireApproval,
            AutomationPatchPermissionDto::AutoApprove => AgentPatchPermission::AutoApprove,
        },
        builtin_execution: match projection.builtin_execution {
            AutomationBuiltinExecutionPermissionDto::RequireApproval => {
                AgentBuiltinExecutionPermission::RequireApproval
            }
            AutomationBuiltinExecutionPermissionDto::AutoApprove => {
                AgentBuiltinExecutionPermission::AutoApprove
            }
        },
    }
}

/// Read-only projection used by new-chat automation DTOs. Reasoning remains model-owned;
/// there is deliberately no independent automation override.
pub(crate) fn reasoning_projection(model: &ModelConfigRecord) -> AutomationReasoningProjectionDto {
    let mode = model.provider_profile_config.reasoning_mode();
    let effort = model.provider_profile_config.provider_reasoning_effort();
    AutomationReasoningProjectionDto {
        source: AutomationReasoningSourceDto::ModelConfig,
        mode: match mode {
            ReasoningMode::ProviderDefault => AutomationReasoningModeDto::ProviderDefault,
            ReasoningMode::Enabled => AutomationReasoningModeDto::Enabled,
            ReasoningMode::Disabled => AutomationReasoningModeDto::Disabled,
        },
        effort: match effort {
            ProviderReasoningEffort::ProviderDefault => {
                AutomationReasoningEffortDto::ProviderDefault
            }
            ProviderReasoningEffort::Low => AutomationReasoningEffortDto::Low,
            ProviderReasoningEffort::High => AutomationReasoningEffortDto::High,
            ProviderReasoningEffort::Max => AutomationReasoningEffortDto::Max,
        },
    }
}

fn default_chat_permissions() -> AgentPermissions {
    AgentPermissions {
        read: AgentReadPermission::WorkspaceOnly,
        write: AgentWritePermission::WorkspaceOnly,
        command: AgentCommandPermission::RequireApproval,
        command_safety: AgentCommandSafetyPolicy::Guarded,
        patch: AgentPatchPermission::RequireApproval,
        builtin_execution: AgentBuiltinExecutionPermission::RequireApproval,
    }
}

fn full_chat_permissions() -> AgentPermissions {
    AgentPermissions {
        read: AgentReadPermission::All,
        write: AgentWritePermission::All,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::FullAccess,
        patch: AgentPatchPermission::AutoApprove,
        builtin_execution: AgentBuiltinExecutionPermission::AutoApprove,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::provider_profile::{
        ProviderFamilyReasoningPolicy, ProviderFamilySettings, ProviderProfileConfig,
        ProviderProfileRef, ProviderVendorId,
    };

    fn preferences() -> UiPreferencesRecord {
        UiPreferencesRecord {
            profile_avatar_data_url: None,
            profile_display_name: String::new(),
            profile_handle: "USER".to_string(),
            sidebar_conversation_sort: "updated".to_string(),
            sidebar_project_sort: "created".to_string(),
            sidebar_project_order: Vec::new(),
            sidebar_section_order: "projects_first".to_string(),
            native_font_smoothing: false,
            show_token_usage_details: true,
            show_context_window_usage: true,
            translucent_sidebar: false,
            translucent_sidebar_transparency: 54,
            full_permission_enabled: true,
            custom_permission_enabled: true,
            custom_permissions: AgentPermissions {
                read: AgentReadPermission::All,
                write: AgentWritePermission::Denied,
                command: AgentCommandPermission::AutoApprove,
                command_safety: AgentCommandSafetyPolicy::FullAccess,
                patch: AgentPatchPermission::AutoApprove,
                builtin_execution: AgentBuiltinExecutionPermission::AutoApprove,
            },
            updated_at: 0,
        }
    }

    #[test]
    fn permission_modes_match_the_typescript_composer_shared_golden() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../packages/protocol/fixtures/automation-contract-v1.json"
        ))
        .unwrap();
        let golden = &fixture["permissionGolden"];
        let preferences = preferences();
        let custom_projection: AutomationResolvedPermissionsDto =
            serde_json::from_value(golden["customPermissions"].clone()).unwrap();
        assert_eq!(
            permissions_projection(preferences.custom_permissions),
            custom_projection
        );

        for (name, mode) in [
            ("default", AutomationPermissionModeDto::Default),
            ("full", AutomationPermissionModeDto::Full),
            ("custom", AutomationPermissionModeDto::Custom),
        ] {
            let expected: AutomationResolvedPermissionsDto =
                serde_json::from_value(golden["resolved"][name].clone()).unwrap();
            let actual = resolve_automation_permissions(
                mode,
                AUTOMATION_PERMISSION_MODE_VERSION,
                &preferences,
            )
            .unwrap();
            assert_eq!(
                actual.projection, expected,
                "permission mode {name} drifted"
            );
        }
    }

    #[test]
    fn default_mode_exactly_matches_composer_and_not_agent_default() {
        let resolved = resolve_automation_permissions(
            AutomationPermissionModeDto::Default,
            AUTOMATION_PERMISSION_MODE_VERSION,
            &preferences(),
        )
        .unwrap();

        assert_eq!(resolved.permissions, default_chat_permissions());
        assert_eq!(
            resolved.permissions.write,
            AgentWritePermission::WorkspaceOnly
        );
        assert_eq!(
            AgentPermissions::default().write,
            AgentWritePermission::Denied,
            "this assertion guards the intentional semantic difference"
        );
        assert_eq!(
            resolved.projection,
            AutomationResolvedPermissionsDto {
                read: AutomationReadPermissionDto::WorkspaceOnly,
                write: AutomationWritePermissionDto::WorkspaceOnly,
                command: AutomationCommandPermissionDto::RequireApproval,
                command_safety: AutomationCommandSafetyPolicyDto::Guarded,
                patch: AutomationPatchPermissionDto::RequireApproval,
                builtin_execution: AutomationBuiltinExecutionPermissionDto::RequireApproval,
            }
        );
    }

    #[test]
    fn full_mode_exactly_matches_composer() {
        let resolved = resolve_automation_permissions(
            AutomationPermissionModeDto::Full,
            AUTOMATION_PERMISSION_MODE_VERSION,
            &preferences(),
        )
        .unwrap();

        assert_eq!(resolved.permissions, full_chat_permissions());
        assert_eq!(resolved.projection.read, AutomationReadPermissionDto::All);
        assert_eq!(resolved.projection.write, AutomationWritePermissionDto::All);
        assert_eq!(
            resolved.projection.command,
            AutomationCommandPermissionDto::AutoApprove
        );
        assert_eq!(
            resolved.projection.command_safety,
            AutomationCommandSafetyPolicyDto::FullAccess
        );
        assert_eq!(
            resolved.projection.patch,
            AutomationPatchPermissionDto::AutoApprove
        );
        assert_eq!(
            resolved.projection.builtin_execution,
            AutomationBuiltinExecutionPermissionDto::AutoApprove
        );
    }

    #[test]
    fn custom_mode_copies_user_fields_but_forces_guarded_command_safety() {
        let preferences = preferences();
        let resolved = resolve_automation_permissions(
            AutomationPermissionModeDto::Custom,
            AUTOMATION_PERMISSION_MODE_VERSION,
            &preferences,
        )
        .unwrap();

        assert_eq!(resolved.permissions.read, AgentReadPermission::All);
        assert_eq!(resolved.permissions.write, AgentWritePermission::Denied);
        assert_eq!(
            resolved.permissions.command,
            AgentCommandPermission::AutoApprove
        );
        assert_eq!(
            resolved.permissions.command_safety,
            AgentCommandSafetyPolicy::Guarded
        );
        assert_eq!(
            resolved.permissions.patch,
            AgentPatchPermission::AutoApprove
        );
        assert_eq!(
            resolved.permissions.builtin_execution,
            AgentBuiltinExecutionPermission::AutoApprove
        );
    }

    #[test]
    fn disabled_elevated_modes_fail_closed_without_default_fallback() {
        let mut preferences = preferences();
        preferences.full_permission_enabled = false;
        preferences.custom_permission_enabled = false;

        assert_eq!(
            resolve_automation_permissions(
                AutomationPermissionModeDto::Full,
                AUTOMATION_PERMISSION_MODE_VERSION,
                &preferences,
            )
            .unwrap_err(),
            AutomationPermissionResolveError::FullPermissionDisabled
        );
        assert_eq!(
            resolve_automation_permissions(
                AutomationPermissionModeDto::Custom,
                AUTOMATION_PERMISSION_MODE_VERSION,
                &preferences,
            )
            .unwrap_err(),
            AutomationPermissionResolveError::CustomPermissionDisabled
        );
        assert_eq!(
            ensure_automation_permission_mode_enabled(
                AutomationPermissionModeDto::Default,
                &preferences
            ),
            Ok(())
        );
    }

    #[test]
    fn unsupported_mode_version_is_rejected() {
        let error = resolve_automation_permissions(
            AutomationPermissionModeDto::Default,
            AUTOMATION_PERMISSION_MODE_VERSION + 1,
            &preferences(),
        )
        .unwrap_err();

        assert_eq!(error.code(), "unsupported_permission_mode_version");
        assert_eq!(
            error,
            AutomationPermissionResolveError::UnsupportedModeVersion {
                requested: AUTOMATION_PERMISSION_MODE_VERSION + 1,
                supported: AUTOMATION_PERMISSION_MODE_VERSION,
            }
        );
    }

    #[test]
    fn run_time_enablement_check_does_not_re_resolve_custom_snapshot() {
        let mut preferences = preferences();
        assert_eq!(
            ensure_automation_permission_mode_enabled(
                AutomationPermissionModeDto::Custom,
                &preferences
            ),
            Ok(())
        );
        preferences.custom_permission_enabled = false;
        assert_eq!(
            ensure_automation_permission_mode_enabled(
                AutomationPermissionModeDto::Custom,
                &preferences
            ),
            Err(AutomationPermissionResolveError::CustomPermissionDisabled)
        );
    }

    #[test]
    fn reasoning_is_a_read_only_model_config_projection() {
        let mut model = ModelConfigRecord {
            id: "deepseek-flash".to_string(),
            provider_model_id: "deepseek-flash".to_string(),
            display_name: "DeepSeek Flash".to_string(),
            api_url_override: None,
            api_token_override: None,
            supports_image: true,
            context_window_tokens: None,
            provider_profile_config: ProviderProfileConfig::from_family_settings(
                ProviderProfileRef::deepseek_v4_1_flash_chat(),
                ProviderVendorId::DeepSeek,
                ProviderFamilySettings::DeepseekFlashChat {
                    reasoning: ProviderFamilyReasoningPolicy {
                        mode: ReasoningMode::Enabled,
                        effort: ProviderReasoningEffort::Max,
                    },
                },
            ),
            input_price: String::new(),
            cached_input_price: String::new(),
            output_price: String::new(),
            enabled: true,
        };

        assert_eq!(
            reasoning_projection(&model),
            AutomationReasoningProjectionDto {
                source: AutomationReasoningSourceDto::ModelConfig,
                mode: AutomationReasoningModeDto::Enabled,
                effort: AutomationReasoningEffortDto::Max,
            }
        );

        model.provider_profile_config = ProviderProfileConfig::from_family_settings(
            ProviderProfileRef::deepseek_v4_1_flash_chat(),
            ProviderVendorId::DeepSeek,
            ProviderFamilySettings::DeepseekFlashChat {
                reasoning: ProviderFamilyReasoningPolicy {
                    mode: ReasoningMode::Enabled,
                    effort: ProviderReasoningEffort::High,
                },
            },
        );
        assert_eq!(
            reasoning_projection(&model).effort,
            AutomationReasoningEffortDto::High
        );

        model.provider_profile_config = ProviderProfileConfig::from_family_settings(
            ProviderProfileRef::deepseek_v4_1_flash_chat(),
            ProviderVendorId::DeepSeek,
            ProviderFamilySettings::DeepseekFlashChat {
                reasoning: ProviderFamilyReasoningPolicy {
                    mode: ReasoningMode::Enabled,
                    effort: ProviderReasoningEffort::Low,
                },
            },
        );
        assert_eq!(
            reasoning_projection(&model).effort,
            AutomationReasoningEffortDto::Low
        );
    }
}
