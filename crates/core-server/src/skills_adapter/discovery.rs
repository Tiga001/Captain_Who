use super::*;
use mycopilot_core::skills::AgentSkillDiscoverySnapshot;

/// Freezes the complete globally enabled Skill catalog for one Agent run.
///
/// Workspace Skills are intentionally absent: the global Settings page does not govern repository
/// content, so those Skills remain explicit-only until a separate project policy is introduced.
/// Catalog truncation and registered-source failures are fail-closed because they make discovery
/// incomplete. Diagnostics for one invalid installed receipt or immutable package stay isolated;
/// the remaining verified descriptors can still be exposed deterministically.
pub(crate) fn prepare_enabled_skill_discovery(
    storage: &StorageService,
    service: &SkillsService,
    effective_context_window_tokens: u32,
) -> Result<Option<AgentSkillDiscoverySnapshot>, String> {
    let catalog = service
        .list()
        .map_err(|error| format!("Cannot discover enabled Skills: {error}"))?;
    if catalog.truncated() {
        return Err(
            "The Skill catalog was truncated and cannot be safely exposed to the Agent. Remove invalid or excessive installations and retry."
                .to_string(),
        );
    }
    let fatal_source_diagnostic_count = catalog
        .diagnostics()
        .iter()
        .filter(|diagnostic| {
            diagnostic.severity() == SkillDiagnosticSeverity::Error
                && service.is_registered_source_id(diagnostic.path())
                && matches!(
                    diagnostic.code(),
                    SkillDiagnosticCode::SourceUnavailable
                        | SkillDiagnosticCode::SourceContractViolation
                )
        })
        .count();
    if fatal_source_diagnostic_count > 0 {
        return Err(format!(
            "The Skill catalog contains {fatal_source_diagnostic_count} source-level error(s) and cannot be safely exposed to the Agent. Review the Skill diagnostics in Settings and retry."
        ));
    }
    let managed = catalog
        .skills()
        .iter()
        .filter(|skill| {
            matches!(
                skill.source_kind(),
                SkillSourceKind::Bundled | SkillSourceKind::Installed
            )
        })
        .collect::<Vec<_>>();
    let managed_ids = managed
        .iter()
        .map(|skill| skill.id().as_str().to_string())
        .collect::<Vec<_>>();
    let enablement = storage
        .load_skill_enablement(&managed_ids)
        .map_err(|error| format!("Cannot load Skill enablement for Agent discovery: {error}"))?;
    let enabled = managed
        .into_iter()
        .filter(|skill| enablement.get(skill.id().as_str()) == Some(&true))
        .collect::<Vec<_>>();
    if enabled.is_empty() {
        return Ok(None);
    }
    let revision = enabled_catalog_revision(&catalog, &enablement);
    let policy = service.activation_policy();
    AgentSkillDiscoverySnapshot::from_descriptors(
        revision,
        enabled,
        Some(effective_context_window_tokens),
        policy.max_skills(),
        policy.max_total_source_bytes(),
    )
    .map(Some)
    .map_err(|error| format!("Cannot prepare the Agent Skill catalog: {error}"))
}
