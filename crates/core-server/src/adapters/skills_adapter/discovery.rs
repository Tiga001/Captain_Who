use super::*;
use mycopilot_core::skills::AgentSkillDiscoverySnapshot;

/// Freezes the complete enabled Skill catalog for one Agent run. Workspace Skills belong only to
/// the current project and participate without global Settings enablement.
/// Catalog truncation and registered-source failures are fail-closed because they make discovery
/// incomplete. Diagnostics for one invalid installed receipt or immutable package stay isolated;
/// the remaining verified descriptors can still be exposed deterministically.
pub(crate) fn prepare_enabled_skill_discovery(
    storage: &StorageService,
    service: &SkillsService,
    workspace: Option<(&str, &Path)>,
    effective_context_window_tokens: u32,
) -> Result<Option<AgentSkillDiscoverySnapshot>, String> {
    let workspace_source = workspace
        .map(|(workspace_id, workspace_root)| {
            SkillsService::for_workspace(workspace_id, workspace_root)
                .map_err(|error| format!("Cannot register Workspace Skill discovery: {error}"))
        })
        .transpose()?;
    let catalog = match workspace {
        Some((workspace_id, workspace_root)) => service
            .list_with_workspace(workspace_id, workspace_root)
            .map_err(|error| format!("Cannot register Workspace Skill discovery: {error}"))?,
        None => service
            .list()
            .map_err(|error| format!("Cannot discover enabled Skills: {error}"))?,
    };
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
                && (service.is_registered_source_id(diagnostic.path())
                    || workspace_source
                        .as_ref()
                        .is_some_and(|source| source.is_registered_source_id(diagnostic.path())))
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
        .chain(
            catalog
                .skills()
                .iter()
                .filter(|skill| skill.source_kind() == SkillSourceKind::Workspace),
        )
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
