use super::*;
use crate::adapters::skills_test_support::write_installed_skill;
use mycopilot_core::skills::{
    GitHubAcquisitionTransport, GitHubArchive, GitHubArchiveRequest, GitHubResolveRequest,
    GitHubSkillAcquirer, GitHubTransportError, GitHubWorkflowAcquisitionAdapter,
    PreparedSkillPackage, SkillInstallationAuthority, SkillInstallationId,
    SkillInstallationProvenance, SkillInstallationRefresh, SkillPackageOrigin,
};
use std::sync::Arc;

const FIRST_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const SECOND_COMMIT: &str = "89abcdef0123456789abcdef0123456789abcdef";

#[test]
fn agent_discovery_freezes_every_enabled_managed_skill_with_deterministic_refs() {
    const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b335";
    let fixture = tempfile::tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let store_root = fixture.path().join("skills");
    write_installed_skill(
        &store_root,
        INSTALLATION_ID,
        concat!(
            "---\n",
            "name: installed-discovery-test\n",
            "description: Verify installed Skill discovery.\n",
            "---\n",
            "# Instructions\n",
            "Inspect the requested evidence.\n"
        ),
    );
    let service = SkillsService::new()
        .with_bundled_source()
        .unwrap()
        .with_installed_source(&store_root)
        .unwrap();

    let first = prepare_enabled_skill_discovery(&storage, &service, None, 128_000)
        .unwrap()
        .unwrap();
    let second = prepare_enabled_skill_discovery(&storage, &service, None, 128_000)
        .unwrap()
        .unwrap();

    assert_eq!(first, second);
    assert_eq!(first.skills.len(), service.list().unwrap().skills().len());
    assert!(first
        .skills
        .iter()
        .any(|skill| skill.id == format!("installed:user:{INSTALLATION_ID}")));
    assert!(first
        .skills
        .iter()
        .all(|skill| matches!(skill.source_kind.as_str(), "bundled" | "installed")));
    assert!(first.skills.windows(2).all(|pair| pair[0].id < pair[1].id));
    assert!(first
        .skills
        .iter()
        .all(|skill| skill.activation_ref.starts_with("s_") && skill.activation_ref.len() == 26));
}

#[test]
fn workspace_skill_is_auto_discovered_with_full_resources_and_exact_activation() {
    let fixture = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let workspace = fixture.path().join("workspace");
    let skill = workspace
        .join(".agents")
        .join("skills")
        .join("project-auditor");
    std::fs::create_dir_all(skill.join("references")).unwrap();
    std::fs::create_dir_all(skill.join("assets")).unwrap();
    std::fs::create_dir_all(skill.join("templates")).unwrap();
    std::fs::create_dir_all(skill.join("scripts")).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: project-auditor\ndescription: Audit this project.\n---\n# Audit\nUse the bundled project evidence.\n",
    )
    .unwrap();
    let reference = skill.join("references/guide.md");
    std::fs::write(&reference, "workspace reference v1").unwrap();
    std::fs::write(skill.join("assets/icon.bin"), [1_u8, 2, 3]).unwrap();
    std::fs::write(skill.join("templates/report.md"), "report template").unwrap();
    std::fs::write(skill.join("scripts/check.py"), "print('ok')\n").unwrap();
    let service = Arc::new(SkillsService::new());

    let discovery = prepare_enabled_skill_discovery(
        &storage,
        &service,
        Some(("project-1", workspace.as_path())),
        128_000,
    )
    .unwrap()
    .unwrap();
    assert_eq!(discovery.skills.len(), 1);
    let entry = &discovery.skills[0];
    assert_eq!(entry.source_kind, "workspace");
    assert_eq!(entry.name, "project-auditor");

    let resolver = model_skill_activation_resolver(
        Arc::clone(&storage),
        Arc::clone(&service),
        Some(("project-1".to_string(), workspace.clone())),
    );
    let selection = SkillSelection::parse(entry.id.clone(), entry.revision.clone()).unwrap();
    let resolved = resolver(&selection).unwrap();
    let resources = resolved.skill.resources.as_ref().unwrap();
    assert_eq!(resources.resource_count, 4);
    assert_eq!(
        resources.kinds,
        vec!["asset", "other", "reference", "script"]
    );
    assert_eq!(resolved.resources.package_uris().len(), 1);

    std::fs::write(&reference, "workspace reference v2").unwrap();
    let stale = resolver(&selection).unwrap_err();
    assert_eq!(stale.code(), Some("skill.activationFailed"));
    let changed = prepare_enabled_skill_discovery(
        &storage,
        &service,
        Some(("project-1", workspace.as_path())),
        128_000,
    )
    .unwrap()
    .unwrap();
    assert_ne!(changed.skills[0].revision, entry.revision);
    assert_ne!(changed.skills[0].activation_ref, entry.activation_ref);
}

#[test]
fn agent_discovery_isolates_a_broken_receipt_and_keeps_valid_skills() {
    const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b338";
    let fixture = tempfile::tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let store_root = fixture.path().join("skills");
    write_installed_skill(
        &store_root,
        INSTALLATION_ID,
        concat!(
            "---\n",
            "name: valid-alongside-broken-receipt\n",
            "description: Remain discoverable when another receipt is invalid.\n",
            "---\n",
            "# Instructions\n",
            "Inspect the requested evidence.\n"
        ),
    );
    let receipts = store_root.join("installations");
    std::fs::create_dir_all(&receipts).unwrap();
    std::fs::write(receipts.join("broken.json"), b"{}").unwrap();
    let service = SkillsService::new()
        .with_bundled_source()
        .unwrap()
        .with_installed_source(&store_root)
        .unwrap();

    let catalog = service.list().unwrap();
    assert!(catalog.diagnostics().iter().any(|diagnostic| {
        diagnostic.severity() == SkillDiagnosticSeverity::Error
            && diagnostic.code() == SkillDiagnosticCode::InvalidInstallationReceipt
    }));
    let discovery = prepare_enabled_skill_discovery(&storage, &service, None, 128_000)
        .unwrap()
        .unwrap();

    assert!(discovery
        .skills
        .iter()
        .any(|skill| skill.id == format!("installed:user:{INSTALLATION_ID}")));
    assert!(discovery
        .skills
        .iter()
        .any(|skill| skill.source_kind == "bundled"));
}

#[test]
fn agent_discovery_rejects_an_unavailable_registered_source() {
    let fixture = tempfile::tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let store_root = fixture.path().join("skills-as-file");
    std::fs::write(&store_root, b"not a directory").unwrap();
    let service = SkillsService::new()
        .with_bundled_source()
        .unwrap()
        .with_installed_source(&store_root)
        .unwrap();

    let catalog = service.list().unwrap();
    assert!(catalog.diagnostics().iter().any(|diagnostic| {
        diagnostic.code() == SkillDiagnosticCode::SourceUnavailable
            && diagnostic.path() == "installed:user"
    }));
    let error = prepare_enabled_skill_discovery(&storage, &service, None, 128_000).unwrap_err();

    assert!(error.contains("source-level error"));
}

#[test]
fn agent_discovery_keeps_complete_catalogs_with_nonfatal_warnings() {
    const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b336";
    const SECOND_INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b337";
    let fixture = tempfile::tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let store_root = fixture.path().join("skills");
    let source = concat!(
        "---\n",
        "name: shared-warning-name\n",
        "description: Verify warning-tolerant discovery.\n",
        "---\n",
        "# Instructions\n",
        "Inspect the requested evidence.\n"
    );
    write_installed_skill(&store_root, INSTALLATION_ID, source);
    write_installed_skill(&store_root, SECOND_INSTALLATION_ID, source);
    let service = SkillsService::new()
        .with_installed_source(&store_root)
        .unwrap();

    let catalog = service.list().unwrap();
    assert!(catalog
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.severity() == SkillDiagnosticSeverity::Warning));
    let discovery = prepare_enabled_skill_discovery(&storage, &service, None, 128_000)
        .unwrap()
        .unwrap();

    assert_eq!(discovery.skills.len(), 2);
    assert!(discovery
        .skills
        .iter()
        .any(|skill| skill.id == format!("installed:user:{INSTALLATION_ID}")));
    assert!(discovery
        .skills
        .iter()
        .any(|skill| skill.id == format!("installed:user:{SECOND_INSTALLATION_ID}")));
}

#[test]
fn agent_discovery_honors_settings_enablement_and_revises_the_frozen_catalog() {
    let fixture = tempfile::tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let service = SkillsService::new().with_bundled_source().unwrap();
    let descriptor = service.list().unwrap().skills()[0].clone();

    let enabled = prepare_enabled_skill_discovery(&storage, &service, None, 128_000)
        .unwrap()
        .unwrap();
    assert!(enabled
        .skills
        .iter()
        .any(|skill| skill.id == descriptor.id().as_str()));

    storage
        .set_skill_enablement_override(descriptor.id().as_str(), false)
        .unwrap();
    let disabled = prepare_enabled_skill_discovery(&storage, &service, None, 128_000)
        .unwrap()
        .unwrap();
    assert!(disabled
        .skills
        .iter()
        .all(|skill| skill.id != descriptor.id().as_str()));
    assert_ne!(disabled.catalog_revision, enabled.catalog_revision);

    storage
        .set_skill_enablement_override(descriptor.id().as_str(), true)
        .unwrap();
    let restored = prepare_enabled_skill_discovery(&storage, &service, None, 128_000)
        .unwrap()
        .unwrap();
    assert_eq!(restored, enabled);

    for skill in service.list().unwrap().skills() {
        storage
            .set_skill_enablement_override(skill.id().as_str(), false)
            .unwrap();
    }
    assert!(
        prepare_enabled_skill_discovery(&storage, &service, None, 128_000)
            .unwrap()
            .is_none()
    );
}

#[test]
fn model_activation_resolver_rechecks_enablement_after_discovery_is_frozen() {
    let fixture = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = Arc::new(SkillsService::new().with_bundled_source().unwrap());
    let discovery = prepare_enabled_skill_discovery(&storage, &service, None, 128_000)
        .unwrap()
        .unwrap();
    let entry = discovery.skills.first().unwrap();
    let selection = SkillSelection::parse(entry.id.clone(), entry.revision.clone()).unwrap();
    let resolver =
        model_skill_activation_resolver(Arc::clone(&storage), Arc::clone(&service), None);

    let resolved = resolver(&selection).unwrap();
    assert_eq!(resolved.skill.id, entry.id);
    assert_eq!(resolved.skill.revision, entry.revision);

    storage
        .set_skill_enablement_override(&entry.id, false)
        .unwrap();
    let error = resolver(&selection).unwrap_err();
    assert_eq!(error.code(), Some("skill.disabled"));
    assert!(error.details().is_some_and(|details| {
        details["type"] == "skillActivation"
            && details["code"] == "skill.disabled"
            && details["recovery"] == "enableSkill"
    }));
}

struct NeverGitHubTransport;

impl GitHubAcquisitionTransport for NeverGitHubTransport {
    fn resolve_commit(
        &self,
        _request: &GitHubResolveRequest,
    ) -> Result<mycopilot_core::skills::GitHubCommit, GitHubTransportError> {
        Err(GitHubTransportError::Unavailable)
    }

    fn download_archive(
        &self,
        _request: &GitHubArchiveRequest,
    ) -> Result<GitHubArchive, GitHubTransportError> {
        Err(GitHubTransportError::Unavailable)
    }
}

fn management_github_provenance(resolved_commit: &str) -> SkillInstallationProvenance {
    let authority = serde_json::json!({
        "owner": "example",
        "repository": "skills",
        "resolvedCommit": resolved_commit,
        "subdirectory": "auditor"
    })
    .to_string();
    let refresh = serde_json::json!({
        "owner": "example",
        "repository": "skills",
        "reference": { "kind": "named", "value": "main" },
        "subdirectory": "auditor"
    })
    .to_string();
    SkillInstallationProvenance::new(
        SkillInstallationAuthority::new("github", 1, authority).unwrap(),
        Some(SkillInstallationRefresh::new("github", 1, refresh).unwrap()),
    )
}

#[test]
fn picker_catalog_filters_disabled_global_skills_and_revises_its_etag() {
    let fixture = tempfile::tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let service = SkillsService::new().with_bundled_source().unwrap();
    let catalog = service.list().unwrap();
    let descriptor = catalog.skills().first().unwrap();

    let enabled = enabled_catalog_response(&storage, &catalog).unwrap();
    assert_eq!(enabled.skills.len(), 6);
    assert_ne!(enabled.catalog_revision, catalog.catalog_revision());

    storage
        .set_skill_enablement_override(descriptor.id().as_str(), false)
        .unwrap();
    let disabled = enabled_catalog_response(&storage, &catalog).unwrap();
    assert_eq!(disabled.skills.len(), 5);
    assert!(disabled
        .skills
        .iter()
        .all(|skill| skill.id != descriptor.id().as_str()));
    assert_ne!(disabled.catalog_revision, enabled.catalog_revision);

    storage
        .set_skill_enablement_override(descriptor.id().as_str(), true)
        .unwrap();
    let restored = enabled_catalog_response(&storage, &catalog).unwrap();
    assert_eq!(restored.skills.len(), 6);
    assert_eq!(restored.catalog_revision, enabled.catalog_revision);
}

#[test]
fn bundled_activation_crosses_schema_v4_as_an_opaque_selection() {
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
fn installed_activation_crosses_schema_v4_as_untrusted_opaque_selection() {
    const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b334";
    const INSTRUCTION_MARKER: &str = "INSTALLED_SKILL_RUNTIME_MARKER";
    let fixture = tempfile::tempdir().unwrap();
    let store_root = fixture.path().join("skills");
    let source_text = format!(
        concat!(
            "---\n",
            "name: installed-auditor\n",
            "description: Audit a repository from an installed package.\n",
            "---\n",
            "# Instructions\n",
            "{}\n"
        ),
        INSTRUCTION_MARKER
    );
    let revision = write_installed_skill(&store_root, INSTALLATION_ID, &source_text);
    let service = SkillsService::new()
        .with_installed_source(&store_root)
        .unwrap();
    let catalog = service.list().unwrap();
    assert!(catalog.diagnostics().is_empty());
    let descriptor = catalog.skills().first().unwrap();
    assert_eq!(
        descriptor.id().as_str(),
        format!("installed:user:{INSTALLATION_ID}")
    );
    assert_eq!(descriptor.revision().as_str(), revision);

    let response = catalog_response(&catalog).unwrap();
    assert_eq!(response.schema_version, 4);
    assert_eq!(
        response.skills[0].source.kind,
        SkillSourceKindDto::Installed
    );
    assert_eq!(response.skills[0].source.id, "installed:user");
    assert_eq!(response.skills[0].trust, SkillTrustDto::Untrusted);

    let prepared = activate_workspace(
        &service,
        "project-1",
        fixture.path(),
        &[SkillSelectionDto {
            id: descriptor.id().as_str().to_string(),
            revision,
        }],
    )
    .unwrap();

    assert_eq!(prepared.summaries.len(), 1);
    assert_eq!(
        prepared.summaries[0].source.kind,
        SkillSourceKindDto::Installed
    );
    assert_eq!(prepared.summaries[0].source.id, "installed:user");
    let runtime = prepared.runtime.unwrap();
    assert_eq!(runtime.skills.len(), 1);
    assert_eq!(runtime.skills[0].source, "installed:user");
    assert!(runtime.skills[0].instructions.contains(INSTRUCTION_MARKER));
}

#[test]
fn schema_v4_rejects_domain_trust_not_represented_by_the_protocol() {
    let error = trust_dto(SkillTrust::UserApproved).unwrap_err();

    assert!(error.contains("userApproved"));
    assert!(error.contains("schema v4"));
}

#[test]
fn schema_v4_does_not_treat_installation_as_application_trust() {
    assert!(supports_protocol_contract(
        SkillSourceKindDto::Installed,
        SkillTrustDto::Untrusted
    ));
    assert!(!supports_protocol_contract(
        SkillSourceKindDto::Installed,
        SkillTrustDto::Application
    ));
}

#[test]
fn installation_io_details_are_not_exposed_across_json_rpc() {
    use mycopilot_core::skills::{SkillId, SkillInstallationId};

    const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b334";
    const PRIVATE_PATH: &str = "/Users/private-account/Library/Application Support/skills";
    let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
    let skill_id = SkillId::parse(format!("installed:user:{INSTALLATION_ID}")).unwrap();
    let error = SkillInstallationServiceError::Installer {
        operation: SkillInstallationOperation::Install,
        installation_id: installation_id.clone(),
        skill_id,
        source: Box::new(ManagedSkillInstallerError::Io {
            operation: format!("publish package under {PRIVATE_PATH}"),
            reason: format!("permission denied while syncing {PRIVATE_PATH}"),
        }),
    };

    let failure = installation_failure(&error).unwrap();
    let public_message = failure.to_string();
    let data = failure.into_data();
    let serialized = serde_json::to_string(&data).unwrap();

    assert_eq!(data.code, SkillInstallationErrorCodeDto::Io);
    assert_eq!(
        public_message,
        "The Skill operation could not be completed. Retry the same request."
    );
    assert_eq!(data.message, public_message);
    assert!(!serialized.contains(PRIVATE_PATH));
    assert!(!serialized.contains("private-account"));
    assert!(!serialized.contains("permission denied"));
}

#[test]
fn commit_indeterminate_preserves_retry_identity_without_a_path() {
    use mycopilot_core::skills::{
        ManagedSkillMutation, SkillId, SkillInstallationId, SkillInstallationRevision,
    };

    const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b334";
    let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
    let skill_id = SkillId::parse(format!("installed:user:{INSTALLATION_ID}")).unwrap();
    let intended_revision = SkillInstallationRevision::parse(format!(
        "skill-installation-sha256-v1:{}",
        "a".repeat(64)
    ))
    .unwrap();
    let error = SkillInstallationServiceError::Installer {
        operation: SkillInstallationOperation::Update,
        installation_id: installation_id.clone(),
        skill_id: skill_id.clone(),
        source: Box::new(ManagedSkillInstallerError::CommitIndeterminate {
            operation: ManagedSkillMutation::Update,
            installation_id,
            intended_revision: Some(intended_revision.clone()),
            reason: "receipt directory sync acknowledgement was lost".to_string(),
        }),
    };

    let data = installation_failure(&error).unwrap().into_data();

    assert_eq!(
        data.error_type,
        SkillInstallationErrorTypeDto::SkillInstallation
    );
    assert_eq!(data.operation, SkillInstallationOperationDto::Update);
    assert_eq!(
        data.code,
        SkillInstallationErrorCodeDto::CommitIndeterminate
    );
    assert_eq!(
        data.recovery,
        SkillInstallationRecoveryDto::RetrySameRequest
    );
    assert!(data.commit_may_have_succeeded);
    assert_eq!(data.skill_id.as_deref(), Some(skill_id.as_str()));
    assert_eq!(
        data.intended_revision.as_deref(),
        Some(intended_revision.as_str())
    );
    assert!(!serde_json::to_value(data)
        .unwrap()
        .to_string()
        .contains("path"));
}

#[test]
fn retired_identity_and_ledger_capacity_have_structured_recovery() {
    use mycopilot_core::skills::{SkillId, SkillInstallationId};

    const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b334";
    let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
    let skill_id = SkillId::parse(format!("installed:user:{INSTALLATION_ID}")).unwrap();
    let retired = SkillInstallationServiceError::Installer {
        operation: SkillInstallationOperation::Install,
        installation_id: installation_id.clone(),
        skill_id: skill_id.clone(),
        source: Box::new(ManagedSkillInstallerError::InstallationRetired {
            installation_id: installation_id.clone(),
        }),
    };
    let retired_data = installation_failure(&retired).unwrap().into_data();
    assert_eq!(
        retired_data.code,
        SkillInstallationErrorCodeDto::InstallationRetired
    );
    assert_eq!(
        retired_data.recovery,
        SkillInstallationRecoveryDto::NewInstallationIdentity
    );

    let full = SkillInstallationServiceError::Installer {
        operation: SkillInstallationOperation::Install,
        installation_id,
        skill_id,
        source: Box::new(ManagedSkillInstallerError::CapacityExceeded {
            capacity: ManagedSkillStoreCapacity::RetiredInstallationIds,
            limit: 100_000,
        }),
    };
    let full_data = installation_failure(&full).unwrap().into_data();
    assert_eq!(
        full_data.capacity,
        Some(SkillInstallationCapacityDto::RetiredInstallationIds)
    );
    assert_eq!(
        full_data.recovery,
        SkillInstallationRecoveryDto::ContactSupport
    );
    assert_eq!(full_data.limit, Some(100_000));
}

#[test]
fn non_installed_targets_map_to_a_refreshable_invalid_skill_error() {
    let skill_id = mycopilot_core::skills::SkillId::parse("workspace:project:auditor").unwrap();
    let error = SkillInstallationServiceError::InvalidInstalledSkill {
        operation: SkillInstallationOperation::Uninstall,
        skill_id: skill_id.clone(),
        reason: "not a managed installation".to_string(),
    };

    let data = installation_failure(&error).unwrap().into_data();

    assert_eq!(data.code, SkillInstallationErrorCodeDto::InvalidSkill);
    assert_eq!(data.recovery, SkillInstallationRecoveryDto::RefreshCatalog);
    assert_eq!(data.skill_id.as_deref(), Some(skill_id.as_str()));
    assert!(!data.commit_may_have_succeeded);
}

#[test]
fn management_inventory_tracks_refreshable_source_only_updates() {
    const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b335";
    let fixture = tempfile::tempdir().unwrap();
    let store_root = fixture.path().join("skills");
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let installations = SkillInstallationService::new(&store_root).unwrap();
    let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
    let package = PreparedSkillPackage::from_bytes(
            b"---\nname: managed-auditor\ndescription: Management projection fixture.\n---\n# Instructions\nAudit the repository.\n".to_vec(),
            SkillPackageOrigin::new("github", "resolved-fixture").unwrap(),
        )
        .unwrap();
    let installed = installations
        .install_prepared_with_provenance(
            installation_id.clone(),
            package.clone(),
            management_github_provenance(FIRST_COMMIT),
        )
        .unwrap();
    let mut workflow =
        SkillInstallationWorkflow::new(SkillInstallationService::new(&store_root).unwrap());
    workflow
        .register_adapter(Arc::new(GitHubWorkflowAcquisitionAdapter::new(Arc::new(
            GitHubSkillAcquirer::with_transport(Arc::new(NeverGitHubTransport)),
        ))))
        .unwrap();
    let catalog_service = SkillsService::new()
        .with_installed_source(&store_root)
        .unwrap();
    let catalog = catalog_service.list().unwrap();

    let first = management_response(&storage, &catalog, &installations, Some(&workflow)).unwrap();
    let entry = &first.skills[0];
    assert_eq!(
        entry.installation_revision.as_deref(),
        Some(installed.installation_revision().unwrap().as_str())
    );
    assert!(entry.actions.can_update);
    assert!(entry.actions.can_uninstall);
    match entry.acquisition.as_ref().unwrap() {
        SkillPreviewSourceDto::GithubRepository {
            owner,
            repository,
            reference,
            resolved_commit,
            subdirectory,
            refreshable,
        } => {
            assert_eq!(owner, "example");
            assert_eq!(repository, "skills");
            assert_eq!(
                reference,
                &SkillGithubReferenceDto::Named {
                    value: "main".to_string()
                }
            );
            assert_eq!(resolved_commit, FIRST_COMMIT);
            assert_eq!(subdirectory.as_deref(), Some("auditor"));
            assert!(*refreshable);
        }
        source => panic!("expected GitHub management source, got {source:?}"),
    }
    assert_eq!(
        management_acquisition(
            &catalog.skills()[0],
            &InstalledSkillSourcePresentation::Provider {
                provider: "registry".to_string(),
                display_name: "Example Registry".to_string(),
                refreshable: true,
            },
        ),
        SkillPreviewSourceDto::InstalledSource {
            display_name: "Example Registry".to_string(),
            refreshable: true,
        },
        "future adapters retain their safe display name and refresh capability"
    );

    let updated = installations
        .update_prepared_exact(
            installation_id,
            installed.installation_revision().unwrap().clone(),
            package,
            management_github_provenance(SECOND_COMMIT),
        )
        .unwrap();
    assert_eq!(
        updated.package_revision(),
        installed.package_revision(),
        "the fixture performs a provenance-only update"
    );
    let second = management_response(&storage, &catalog, &installations, Some(&workflow)).unwrap();
    assert_ne!(
        second.skills[0].installation_revision,
        first.skills[0].installation_revision
    );
    assert_ne!(
        second.skills[0].state_revision,
        first.skills[0].state_revision
    );
    assert_ne!(second.management_revision, first.management_revision);
    assert!(matches!(
        second.skills[0].acquisition.as_ref(),
        Some(SkillPreviewSourceDto::GithubRepository {
            resolved_commit,
            refreshable: true,
            ..
        }) if resolved_commit == SECOND_COMMIT
    ));
}
