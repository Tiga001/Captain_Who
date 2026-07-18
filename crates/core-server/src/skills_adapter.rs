//! Explicit boundary between the Skill domain model, runtime snapshots, and JSON-RPC DTOs.

use std::path::Path;

use mycopilot_core::skills::{
    SkillActivationError, SkillActivationScope, SkillCatalog, SkillDescriptor,
    SkillDiagnosticSeverity, SkillErrorCode, SkillProvenance, SkillRecovery, SkillSelection,
    SkillSourceKind, SkillTrust, SkillsService,
};
use mycopilot_core::{AgentActivatedSkill, AgentSkillActivation};
use mycopilot_protocol_rs::{
    ActivatedSkillSummaryDto, SkillActivationErrorData, SkillDescriptorDto, SkillDiagnosticDto,
    SkillSelectionDto, SkillSourceDto, SkillSourceKindDto, SkillTrustDto, SkillsListResponse,
    SKILL_CATALOG_SCHEMA_VERSION,
};

#[derive(Debug, Default)]
pub(crate) struct PreparedSkillActivation {
    pub(crate) runtime: Option<AgentSkillActivation>,
    pub(crate) summaries: Vec<ActivatedSkillSummaryDto>,
    pub(crate) revision: Option<String>,
}

#[derive(Debug)]
pub(crate) struct SkillActivationFailure {
    data: Box<SkillActivationErrorData>,
}

impl SkillActivationFailure {
    pub(crate) fn into_data(self) -> Box<SkillActivationErrorData> {
        self.data
    }

    fn invalid_selection(skill_id: Option<String>, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            data: Box::new(SkillActivationErrorData {
                error_type: "skillActivation",
                code: "invalidSelection".to_string(),
                recovery: "rejectSelection".to_string(),
                message,
                skill_id,
                expected_revision: None,
                actual_revision: None,
            }),
        }
    }
}

impl std::fmt::Display for SkillActivationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.data.message)
    }
}

impl std::error::Error for SkillActivationFailure {}

pub(crate) fn catalog_response(catalog: &SkillCatalog) -> Result<SkillsListResponse, String> {
    Ok(SkillsListResponse {
        schema_version: SKILL_CATALOG_SCHEMA_VERSION,
        catalog_revision: catalog.catalog_revision().to_string(),
        skills: catalog
            .skills()
            .iter()
            .map(descriptor_dto)
            .collect::<Result<Vec<_>, _>>()?,
        diagnostics: catalog
            .diagnostics()
            .iter()
            .map(|diagnostic| SkillDiagnosticDto {
                code: diagnostic.code().stable_name().to_string(),
                severity: match diagnostic.severity() {
                    SkillDiagnosticSeverity::Warning => "warning",
                    SkillDiagnosticSeverity::Error => "error",
                    _ => "error",
                }
                .to_string(),
                message: diagnostic.message().to_string(),
                skill_id: None,
                location: Some(diagnostic.path().to_string()),
            })
            .collect(),
        truncated: catalog.truncated(),
    })
}

pub(crate) fn activate_workspace(
    service: &SkillsService,
    workspace_id: &str,
    workspace_root: &Path,
    selections: &[SkillSelectionDto],
) -> Result<PreparedSkillActivation, SkillActivationFailure> {
    if selections.is_empty() {
        return Ok(PreparedSkillActivation::default());
    }

    let selections = selections
        .iter()
        .map(|selection| {
            SkillSelection::parse(&selection.id, &selection.revision).map_err(|error| {
                SkillActivationFailure::invalid_selection(
                    non_empty(&selection.id),
                    format!("Invalid Skill selection: {error}"),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let activated = service
        .activate_workspace(workspace_id, workspace_root, &selections)
        .map_err(activation_failure)?;

    for skill in activated.skills() {
        validate_protocol_descriptor(skill.descriptor(), Some(workspace_id)).map_err(
            |message| {
                SkillActivationFailure::invalid_selection(
                    Some(skill.id().as_str().to_string()),
                    message,
                )
            },
        )?;
    }

    let revision = activated.revision().as_str().to_string();
    let summaries = activated
        .skills()
        .iter()
        .map(|skill| {
            Ok(ActivatedSkillSummaryDto {
                id: skill.id().as_str().to_string(),
                name: skill.name().to_string(),
                revision: skill.revision().as_str().to_string(),
                source: source_dto(skill.descriptor())?,
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map_err(|message| SkillActivationFailure::invalid_selection(None, message))?;
    let runtime = AgentSkillActivation {
        activation_revision: revision.clone(),
        skills: activated
            .skills()
            .iter()
            .map(|skill| AgentActivatedSkill {
                id: skill.id().as_str().to_string(),
                name: skill.name().to_string(),
                revision: skill.revision().as_str().to_string(),
                source: skill.id().source_id().as_str().to_string(),
                instructions: skill.instructions().to_string(),
            })
            .collect(),
    };

    Ok(PreparedSkillActivation {
        runtime: Some(runtime),
        summaries,
        revision: Some(revision),
    })
}

pub(crate) fn missing_workspace_failure() -> SkillActivationFailure {
    SkillActivationFailure::invalid_selection(
        None,
        "Activating a Skill requires a project with a local workspace.",
    )
}

fn descriptor_dto(descriptor: &SkillDescriptor) -> Result<SkillDescriptorDto, String> {
    let location = validate_protocol_descriptor(descriptor, None)?;
    Ok(SkillDescriptorDto {
        id: descriptor.id().as_str().to_string(),
        name: descriptor.name().to_string(),
        description: descriptor.description().to_string(),
        source: source_dto(descriptor)?,
        trust: trust_dto(descriptor.trust())?,
        activation_scope: activation_scope_name(descriptor.activation_scope())?.to_string(),
        revision: descriptor.revision().as_str().to_string(),
        location,
    })
}

fn source_dto(descriptor: &SkillDescriptor) -> Result<SkillSourceDto, String> {
    let kind = match descriptor.source_kind() {
        SkillSourceKind::Workspace => SkillSourceKindDto::Workspace,
        SkillSourceKind::Bundled => SkillSourceKindDto::Bundled,
        unsupported => {
            return Err(format!(
                "Skill `{}` uses unsupported source kind `{}` for catalog schema v3",
                descriptor.id(),
                unsupported.stable_name()
            ));
        }
    };
    Ok(SkillSourceDto {
        kind,
        id: descriptor.id().source_id().as_str().to_string(),
    })
}

fn trust_dto(trust: SkillTrust) -> Result<SkillTrustDto, String> {
    match trust {
        SkillTrust::Untrusted => Ok(SkillTrustDto::Untrusted),
        SkillTrust::Application => Ok(SkillTrustDto::Application),
        unsupported => Err(format!(
            "unsupported Skill trust `{}` for catalog schema v3",
            unsupported.stable_name()
        )),
    }
}

fn activation_scope_name(scope: SkillActivationScope) -> Result<&'static str, String> {
    match scope {
        SkillActivationScope::Run => Ok("run"),
        unsupported => Err(format!(
            "unsupported Skill activation scope `{}` for catalog schema v3",
            unsupported.stable_name()
        )),
    }
}

fn validate_protocol_descriptor(
    descriptor: &SkillDescriptor,
    expected_workspace_id: Option<&str>,
) -> Result<Option<String>, String> {
    let source_kind = source_dto(descriptor)?.kind;
    let trust = trust_dto(descriptor.trust())?;
    activation_scope_name(descriptor.activation_scope())?;

    let supported_contract = matches!(
        (source_kind, trust),
        (SkillSourceKindDto::Workspace, SkillTrustDto::Untrusted)
            | (SkillSourceKindDto::Bundled, SkillTrustDto::Application)
    );
    if !supported_contract {
        return Err(format!(
            "Skill `{}` cannot cross catalog schema v3 with source `{}` and trust `{}`",
            descriptor.id(),
            descriptor.source_kind().stable_name(),
            descriptor.trust().stable_name()
        ));
    }

    match descriptor.provenance() {
        SkillProvenance::Workspace {
            workspace_id,
            relative_path,
        } if source_kind == SkillSourceKindDto::Workspace
            && expected_workspace_id.is_none_or(|expected| expected == workspace_id) =>
        {
            Ok(Some(relative_path.clone()))
        }
        SkillProvenance::Workspace { .. } => Err(format!(
            "Skill `{}` has workspace provenance incompatible with its source or project",
            descriptor.id()
        )),
        SkillProvenance::Bundled {
            source_id,
            relative_path,
        } if source_kind == SkillSourceKindDto::Bundled
            && source_id == descriptor.id().source_id() =>
        {
            Ok(Some(relative_path.clone()))
        }
        SkillProvenance::Bundled { .. } => Err(format!(
            "Skill `{}` has bundled provenance incompatible with its source",
            descriptor.id()
        )),
        SkillProvenance::Other { .. } => Err(format!(
            "Skill `{}` uses unsupported provenance for catalog schema v3",
            descriptor.id()
        )),
        _ => Err(format!(
            "Skill `{}` uses unknown provenance for catalog schema v3",
            descriptor.id()
        )),
    }
}

fn activation_failure(error: SkillActivationError) -> SkillActivationFailure {
    let code = match error.code() {
        SkillErrorCode::InvalidReference => "invalidSelection",
        SkillErrorCode::DuplicateSelection => "duplicateSelection",
        SkillErrorCode::TooManySkills => "tooManySkills",
        SkillErrorCode::SourceBudgetExceeded => "activationTooLarge",
        SkillErrorCode::NotFound => "notFound",
        SkillErrorCode::Stale => "stale",
        SkillErrorCode::InvalidSkill => "invalidSkill",
        _ => "sourceUnavailable",
    };
    let recovery = match error.recovery() {
        SkillRecovery::Retry => "retrySameSelection",
        SkillRecovery::RefreshCatalog
        | SkillRecovery::RepairSkill
        | SkillRecovery::ReconfigureSource => "refreshCatalog",
        SkillRecovery::ChangeSelection | SkillRecovery::ReduceSelection => "rejectSelection",
        _ => "rejectSelection",
    };
    SkillActivationFailure {
        data: Box::new(SkillActivationErrorData {
            error_type: "skillActivation",
            code: code.to_string(),
            recovery: recovery.to_string(),
            message: error.message(),
            skill_id: error.skill_id().map(|id| id.as_str().to_string()),
            expected_revision: error
                .expected_revision()
                .map(|revision| revision.as_str().to_string()),
            actual_revision: error
                .actual_revision()
                .map(|revision| revision.as_str().to_string()),
        }),
    }
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_activation_crosses_schema_v3_as_an_opaque_selection() {
        let workspace = tempfile::tempdir().unwrap();
        let service = SkillsService::new().with_bundled_source().unwrap();
        let descriptor = service.list().unwrap().skills()[0].clone();
        let prepared = activate_workspace(
            &service,
            "project-1",
            workspace.path(),
            &[SkillSelectionDto {
                id: descriptor.id().as_str().to_string(),
                revision: descriptor.revision().as_str().to_string(),
            }],
        )
        .unwrap();

        assert_eq!(prepared.summaries.len(), 1);
        assert_eq!(
            prepared.summaries[0].source.kind,
            SkillSourceKindDto::Bundled
        );
        assert_eq!(prepared.summaries[0].source.id, "bundled:application");
        let runtime = prepared.runtime.unwrap();
        assert_eq!(runtime.skills.len(), 1);
        assert_eq!(runtime.skills[0].source, "bundled:application");
    }

    #[test]
    fn schema_v3_rejects_domain_trust_not_represented_by_the_protocol() {
        let error = trust_dto(SkillTrust::UserApproved).unwrap_err();

        assert!(error.contains("userApproved"));
        assert!(error.contains("schema v3"));
    }
}
