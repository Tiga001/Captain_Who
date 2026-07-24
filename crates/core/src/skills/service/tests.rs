use super::*;
use crate::skills::digest::package_revision;
use crate::skills::model::{
    SkillActivationScope, SkillDescriptorParts, SkillErrorCode, SkillProvenance, SkillRecovery,
    SkillRevision, SkillSourceKind, SkillTrust, SKILL_PACKAGE_FORMAT_VERSION,
    SKILL_PACKAGE_FORMAT_VERSION_V3,
};
use crate::skills::source::WorkspaceSkillSource;
use crate::skills::workspace::{AGENTS_DIRECTORY, SKILLS_DIRECTORY, SKILL_FILE_NAME};
use crate::skills::DOCUMENTS_LOCAL_ID;
use std::fs;
use std::ops::Range;
use tempfile::tempdir;

#[derive(Debug)]
struct MockSource {
    id: SkillSourceId,
    packages: BTreeMap<super::super::model::SkillId, ResolvedSkillPackage>,
}

impl MockSource {
    fn new(authority: &str, entries: &[(&str, &str, &str)]) -> Self {
        let id = SkillSourceId::parse(format!("bundled:{authority}")).unwrap();
        let mut packages = BTreeMap::new();
        for (local_id, name, instructions) in entries {
            let source_text =
                format!("---\nname: {name}\ndescription: Mock {name}.\n---\n{instructions}");
            let instructions_start = source_text.len() - instructions.len();
            let skill_id = super::super::model::SkillId::from_parts(id.clone(), local_id).unwrap();
            let descriptor = SkillDescriptor::new(SkillDescriptorParts {
                id: skill_id.clone(),
                name: (*name).to_string(),
                description: format!("Mock {name}."),
                source_kind: SkillSourceKind::Bundled,
                trust: SkillTrust::Application,
                activation_scope: SkillActivationScope::Run,
                revision: package_revision(source_text.as_bytes()),
                provenance: SkillProvenance::Bundled {
                    source_id: id.clone(),
                    relative_path: format!("{local_id}/SKILL.md"),
                },
            });
            let range = Range {
                start: instructions_start,
                end: source_text.len(),
            };
            packages.insert(
                skill_id,
                ResolvedSkillPackage::new(descriptor, Arc::from(source_text), range).unwrap(),
            );
        }
        Self { id, packages }
    }
}

impl SkillSource for MockSource {
    fn id(&self) -> &SkillSourceId {
        &self.id
    }

    fn kind(&self) -> SkillSourceKind {
        SkillSourceKind::Bundled
    }

    fn trust(&self) -> super::super::model::SkillTrust {
        super::super::model::SkillTrust::Application
    }

    fn activation_scope(&self) -> SkillActivationScope {
        SkillActivationScope::Run
    }

    fn list(&self) -> Result<SkillCatalog, SkillDiscoveryError> {
        Ok(finalize_catalog(
            self.packages
                .values()
                .map(|package| package.descriptor().clone())
                .collect(),
            Vec::new(),
            false,
        ))
    }

    fn resolve(
        &self,
        selection: &SkillSelection,
    ) -> Result<ResolvedSkillPackage, SkillResolveError> {
        let Some(package) = self.packages.get(selection.skill_id()) else {
            return Err(SkillResolveError::NotFound {
                skill_id: selection.skill_id().clone(),
            });
        };
        if package.revision() != selection.expected_revision() {
            return Err(SkillResolveError::Stale {
                skill_id: selection.skill_id().clone(),
                expected_revision: selection.expected_revision().clone(),
                actual_revision: package.revision().clone(),
            });
        }
        Ok(package.clone())
    }
}

#[derive(Debug)]
struct FailingSource {
    id: SkillSourceId,
}

impl FailingSource {
    fn new(authority: &str) -> Self {
        Self {
            id: SkillSourceId::parse(format!("bundled:{authority}")).unwrap(),
        }
    }
}

impl SkillSource for FailingSource {
    fn id(&self) -> &SkillSourceId {
        &self.id
    }

    fn kind(&self) -> SkillSourceKind {
        SkillSourceKind::Bundled
    }

    fn trust(&self) -> SkillTrust {
        SkillTrust::Application
    }

    fn activation_scope(&self) -> SkillActivationScope {
        SkillActivationScope::Run
    }

    fn list(&self) -> Result<SkillCatalog, SkillDiscoveryError> {
        Err(SkillDiscoveryError::InvalidSource {
            reason: "fixture source is offline".to_string(),
        })
    }

    fn resolve(
        &self,
        selection: &SkillSelection,
    ) -> Result<ResolvedSkillPackage, SkillResolveError> {
        Err(SkillResolveError::Unavailable {
            skill_id: Some(selection.skill_id().clone()),
            reason: "fixture source is offline".to_string(),
        })
    }
}

#[derive(Debug)]
struct LyingResolveSource {
    id: SkillSourceId,
    package: ResolvedSkillPackage,
}

impl SkillSource for LyingResolveSource {
    fn id(&self) -> &SkillSourceId {
        &self.id
    }

    fn kind(&self) -> SkillSourceKind {
        SkillSourceKind::Bundled
    }

    fn trust(&self) -> SkillTrust {
        SkillTrust::Application
    }

    fn activation_scope(&self) -> SkillActivationScope {
        SkillActivationScope::Run
    }

    fn list(&self) -> Result<SkillCatalog, SkillDiscoveryError> {
        Ok(finalize_catalog(
            vec![self.package.descriptor().clone()],
            Vec::new(),
            false,
        ))
    }

    fn resolve(
        &self,
        _selection: &SkillSelection,
    ) -> Result<ResolvedSkillPackage, SkillResolveError> {
        Ok(self.package.clone())
    }
}

fn write_workspace_skill(workspace: &Path, directory: &str) {
    let directory = workspace
        .join(AGENTS_DIRECTORY)
        .join(SKILLS_DIRECTORY)
        .join(directory);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join(SKILL_FILE_NAME),
        "---\nname: workspace-skill\ndescription: Workspace fixture.\n---\n# Run\n",
    )
    .unwrap();
}

#[test]
fn registry_routes_every_descriptor_to_its_owning_source() {
    let workspace = tempdir().unwrap();
    write_workspace_skill(workspace.path(), "workspace-skill");
    let workspace_source =
        Arc::new(WorkspaceSkillSource::new("workspace", workspace.path()).unwrap());
    let bundled_source = Arc::new(MockSource::new(
        "test",
        &[("bundled-skill", "bundled-skill", "# Bundled\n")],
    ));
    let service = SkillsService::from_sources([
        workspace_source as Arc<dyn SkillSource>,
        bundled_source as Arc<dyn SkillSource>,
    ])
    .unwrap();

    let catalog = service.list().unwrap();
    assert_eq!(catalog.skills().len(), 2);
    for descriptor in catalog.skills() {
        let package = service.resolve(&descriptor.selection()).unwrap();
        assert_eq!(package.descriptor(), descriptor);
        assert_eq!(package.format_version(), SKILL_PACKAGE_FORMAT_VERSION);
        assert!(package.resources().is_empty());
    }
    assert!(catalog
        .skills()
        .iter()
        .any(|skill| skill.source_kind() == SkillSourceKind::Bundled));
    assert!(catalog
        .skills()
        .iter()
        .any(|skill| skill.source_kind() == SkillSourceKind::Workspace));
}

#[test]
fn registry_rejects_a_source_id_that_impersonates_another_kind() {
    let mut source = MockSource::new("kind-mismatch", &[("review", "review", "# Review\n")]);
    source.id = SkillSourceId::parse("workspace:kind-mismatch").unwrap();

    let error =
        SkillsService::from_sources([Arc::new(source) as Arc<dyn SkillSource>]).unwrap_err();

    assert_eq!(error.code(), SkillErrorCode::InvalidSource);
    assert!(matches!(
        error,
        SkillRegistrationError::InvalidSource { reason }
            if reason.contains("declares kind `workspace`")
                && reason.contains("reports kind `bundled`")
    ));
}

#[test]
fn catalog_rejects_provenance_that_is_not_bound_to_the_skill_id() {
    let source = MockSource::new("provenance", &[("review", "review", "# Review\n")]);
    let original = source.packages.values().next().unwrap();
    let descriptor = SkillDescriptor::new(SkillDescriptorParts {
        id: original.id().clone(),
        name: original.name().to_string(),
        description: original.description().to_string(),
        source_kind: original.source_kind(),
        trust: original.trust(),
        activation_scope: original.activation_scope(),
        revision: original.revision().clone(),
        provenance: SkillProvenance::Bundled {
            source_id: source.id.clone(),
            relative_path: "another-skill/SKILL.md".to_string(),
        },
    });
    let package = ResolvedSkillPackage::new(
        descriptor,
        Arc::from(original.source_text()),
        original.instructions_range().clone(),
    )
    .unwrap();
    let service = SkillsService::from_sources([Arc::new(LyingResolveSource {
        id: source.id,
        package,
    }) as Arc<dyn SkillSource>])
    .unwrap();

    let catalog = service.list().unwrap();

    assert!(catalog.skills().is_empty());
    assert!(catalog.diagnostics().iter().any(|diagnostic| {
        diagnostic.code() == SkillDiagnosticCode::SourceContractViolation
            && diagnostic.message().contains("another-skill/SKILL.md")
    }));
}

#[test]
fn resolved_contract_binds_revision_to_the_exact_source_snapshot() {
    let source = MockSource::new("digest", &[("review", "review", "# Good\n")]);
    let original = source.packages.values().next().unwrap();
    let tampered_source = original.source_text().replace("# Good\n", "# Evil\n");
    let package = ResolvedSkillPackage::new(
        original.descriptor().clone(),
        Arc::from(tampered_source),
        original.instructions_range().clone(),
    )
    .unwrap();
    let selection = package.descriptor().selection();
    let service = SkillsService::from_sources([Arc::new(LyingResolveSource {
        id: source.id,
        package,
    }) as Arc<dyn SkillSource>])
    .unwrap();

    let error = service.resolve(&selection).unwrap_err();

    assert!(matches!(
        error,
        SkillResolveError::SourceContractViolation { reason, .. }
            if reason.contains("does not match its source snapshot")
    ));
}

#[test]
fn resolved_contract_reparses_metadata_and_instruction_boundaries() {
    let source = MockSource::new("parser", &[("review", "review", "# Review\n")]);
    let original = source.packages.values().next().unwrap();
    let mismatched_descriptor = SkillDescriptor::new(SkillDescriptorParts {
        id: original.id().clone(),
        name: "spoofed-name".to_string(),
        description: original.description().to_string(),
        source_kind: original.source_kind(),
        trust: original.trust(),
        activation_scope: original.activation_scope(),
        revision: original.revision().clone(),
        provenance: original.provenance().clone(),
    });
    let metadata_package = ResolvedSkillPackage::new(
        mismatched_descriptor,
        Arc::from(original.source_text()),
        original.instructions_range().clone(),
    )
    .unwrap();
    let metadata_selection = metadata_package.descriptor().selection();
    let metadata_service = SkillsService::from_sources([Arc::new(LyingResolveSource {
        id: source.id.clone(),
        package: metadata_package,
    }) as Arc<dyn SkillSource>])
    .unwrap();

    let metadata_error = metadata_service.resolve(&metadata_selection).unwrap_err();
    assert!(matches!(
        metadata_error,
        SkillResolveError::SourceContractViolation { reason, .. }
            if reason.contains("metadata name")
    ));

    let shifted_range = original.instructions_range().start + 1..original.instructions_range().end;
    let range_package = ResolvedSkillPackage::new(
        original.descriptor().clone(),
        Arc::from(original.source_text()),
        shifted_range,
    )
    .unwrap();
    let range_selection = range_package.descriptor().selection();
    let range_service = SkillsService::from_sources([Arc::new(LyingResolveSource {
        id: source.id,
        package: range_package,
    }) as Arc<dyn SkillSource>])
    .unwrap();

    let range_error = range_service.resolve(&range_selection).unwrap_err();
    assert!(matches!(
        range_error,
        SkillResolveError::SourceContractViolation { reason, .. }
            if reason.contains("instruction range")
    ));
}

#[test]
fn resolved_bundled_contract_rejects_a_descriptor_supplied_fallback_name() {
    let source_id = SkillSourceId::parse("bundled:missing-name").unwrap();
    let skill_id = super::super::model::SkillId::from_parts(source_id.clone(), "review").unwrap();
    let source_text = "---\ndescription: Missing explicit name.\n---\n# Review\n";
    let instructions_start = source_text.find("# Review").unwrap();
    let descriptor = SkillDescriptor::new(SkillDescriptorParts {
        id: skill_id,
        name: "review".to_string(),
        description: "Missing explicit name.".to_string(),
        source_kind: SkillSourceKind::Bundled,
        trust: SkillTrust::Application,
        activation_scope: SkillActivationScope::Run,
        revision: package_revision(source_text.as_bytes()),
        provenance: SkillProvenance::Bundled {
            source_id: source_id.clone(),
            relative_path: "review/SKILL.md".to_string(),
        },
    });
    let package = ResolvedSkillPackage::new(
        descriptor,
        Arc::from(source_text),
        instructions_start..source_text.len(),
    )
    .unwrap();
    let selection = package.descriptor().selection();
    let service = SkillsService::from_sources([Arc::new(LyingResolveSource {
        id: source_id,
        package,
    }) as Arc<dyn SkillSource>])
    .unwrap();

    let error = service.resolve(&selection).unwrap_err();

    assert!(matches!(
        error,
        SkillResolveError::SourceContractViolation { reason, .. }
            if reason.contains("must declare an explicit name")
    ));
}

#[test]
fn resolved_package_constructor_rejects_invalid_instruction_ranges() {
    let source = MockSource::new("range", &[("review", "review", "# Review\n")]);
    let descriptor = source
        .packages
        .values()
        .next()
        .unwrap()
        .descriptor()
        .clone();

    let reversed_range = Range { start: 3, end: 2 };
    let reversed = ResolvedSkillPackage::new(descriptor.clone(), Arc::from("text"), reversed_range)
        .unwrap_err();
    assert!(reversed.to_string().contains("starts after"));

    let outside =
        ResolvedSkillPackage::new(descriptor.clone(), Arc::from("text"), 0..5).unwrap_err();
    assert!(outside.to_string().contains("exceeds"));

    let utf8 = ResolvedSkillPackage::new(descriptor, Arc::from("é"), 1..2).unwrap_err();
    assert!(utf8.to_string().contains("UTF-8"));
}

#[test]
fn explicit_bundled_registration_exposes_the_real_embedded_skill() {
    let empty = SkillsService::new().list().unwrap();
    assert!(empty.skills().is_empty());

    let service = SkillsService::new().with_bundled_source().unwrap();
    let catalog = service.list().unwrap();

    assert_eq!(catalog.skills().len(), 4);
    let descriptor = catalog
        .skills()
        .iter()
        .find(|skill| skill.id().local_id() == DOCUMENTS_LOCAL_ID)
        .unwrap();
    assert_eq!(descriptor.id().as_str(), "bundled:application:documents");
    assert_eq!(descriptor.source_kind(), SkillSourceKind::Bundled);
    assert_eq!(descriptor.trust(), SkillTrust::Application);
    assert!(catalog.diagnostics().is_empty());

    let package = service.resolve(&descriptor.selection()).unwrap();
    assert_eq!(package.descriptor(), descriptor);
    assert_eq!(package.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V3);
    assert_eq!(package.resources().len(), 3);
    assert!(package.resources().get("office-capability.json").is_some());
    assert!(package.resources().get("references/workflows.md").is_some());
    assert!(package.resources().get("templates/builder.py").is_some());
}

#[test]
fn list_with_workspace_aggregates_real_sources_in_stable_order() {
    let workspace = tempdir().unwrap();
    write_workspace_skill(workspace.path(), "workspace-skill");
    let service = SkillsService::new().with_bundled_source().unwrap();

    let first = service
        .list_with_workspace("workspace", workspace.path())
        .unwrap();
    let second = service
        .list_with_workspace("workspace", workspace.path())
        .unwrap();

    assert_eq!(first, second);
    assert_eq!(first.skills().len(), 5);
    assert_eq!(
        first
            .skills()
            .iter()
            .map(|skill| skill.id().as_str())
            .collect::<Vec<_>>(),
        vec![
            "bundled:application:documents",
            "bundled:application:image-generation",
            "bundled:application:presentations",
            "bundled:application:spreadsheets",
            "workspace:workspace:workspace-skill",
        ]
    );
    assert_eq!(
        first
            .skills()
            .iter()
            .map(SkillDescriptor::source_kind)
            .collect::<Vec<_>>(),
        vec![
            SkillSourceKind::Bundled,
            SkillSourceKind::Bundled,
            SkillSourceKind::Bundled,
            SkillSourceKind::Bundled,
            SkillSourceKind::Workspace,
        ]
    );
    assert!(first.diagnostics().is_empty());

    // The request-scoped workspace must not mutate the registered source set.
    let registered = service.list().unwrap();
    assert_eq!(registered.skills().len(), 4);
    assert!(registered
        .skills()
        .iter()
        .all(|skill| skill.source_kind() == SkillSourceKind::Bundled));
}

#[test]
fn list_with_workspace_rejects_a_registered_workspace_collision() {
    let workspace = tempdir().unwrap();
    write_workspace_skill(workspace.path(), "workspace-skill");
    let service = SkillsService::for_workspace("workspace", workspace.path()).unwrap();

    let error = service
        .list_with_workspace("workspace", workspace.path())
        .unwrap_err();

    assert_eq!(error.code(), SkillErrorCode::DuplicateSource);
    assert!(matches!(
        error,
        SkillRegistrationError::DuplicateSource { source_id }
            if source_id.as_str() == "workspace:workspace"
    ));
    assert_eq!(service.list().unwrap().skills().len(), 1);
}

#[test]
fn real_cross_source_activation_preserves_user_selection_order() {
    let workspace = tempdir().unwrap();
    write_workspace_skill(workspace.path(), "workspace-skill");
    let service = SkillsService::new().with_bundled_source().unwrap();
    let catalog = service
        .list_with_workspace("workspace", workspace.path())
        .unwrap();
    let bundled = catalog
        .skills()
        .iter()
        .find(|skill| skill.source_kind() == SkillSourceKind::Bundled)
        .unwrap()
        .selection();
    let workspace_skill = catalog
        .skills()
        .iter()
        .find(|skill| skill.source_kind() == SkillSourceKind::Workspace)
        .unwrap()
        .selection();

    let activated = service
        .activate_workspace("workspace", workspace.path(), &[workspace_skill, bundled])
        .unwrap();

    assert_eq!(
        activated
            .skills()
            .iter()
            .map(ResolvedSkillPackage::source_kind)
            .collect::<Vec<_>>(),
        vec![SkillSourceKind::Workspace, SkillSourceKind::Bundled]
    );
}

#[test]
fn catalog_keeps_healthy_sources_when_another_source_is_unavailable() {
    let healthy = Arc::new(MockSource::new(
        "healthy",
        &[("review", "review", "# Review\n")],
    ));
    let failing = Arc::new(FailingSource::new("offline"));
    let service = SkillsService::from_sources([
        healthy as Arc<dyn SkillSource>,
        failing as Arc<dyn SkillSource>,
    ])
    .unwrap();

    let catalog = service.list().unwrap();

    assert_eq!(catalog.skills().len(), 1);
    assert_eq!(catalog.skills()[0].name(), "review");
    assert!(catalog
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code() == SkillDiagnosticCode::SourceUnavailable));
}

#[test]
fn registry_rejects_a_source_that_returns_a_different_package() {
    let valid = MockSource::new("lying", &[("returned", "returned", "# Returned\n")]);
    let source_id = valid.id.clone();
    let package = valid.packages.values().next().unwrap().clone();
    let source = Arc::new(LyingResolveSource {
        id: source_id.clone(),
        package,
    });
    let service = SkillsService::from_sources([source as Arc<dyn SkillSource>]).unwrap();
    let requested_id = super::super::model::SkillId::from_parts(source_id, "requested").unwrap();
    let selection = SkillSelection::new(
        requested_id,
        service.list().unwrap().skills()[0].revision().clone(),
    );

    let error = service.resolve(&selection).unwrap_err();

    assert_eq!(error.code(), SkillErrorCode::ResolveFailed);
    assert_eq!(
        error.recovery(),
        super::super::model::SkillRecovery::ReconfigureSource
    );
    assert!(matches!(
        error,
        SkillResolveError::SourceContractViolation { .. }
    ));
}

#[test]
fn request_scoped_workspace_activation_keeps_other_registered_sources() {
    let workspace = tempdir().unwrap();
    write_workspace_skill(workspace.path(), "workspace-skill");
    let bundled_source = Arc::new(MockSource::new(
        "test",
        &[("bundled-skill", "bundled-skill", "# Bundled\n")],
    ));
    let service = SkillsService::from_sources([bundled_source as Arc<dyn SkillSource>]).unwrap();
    let bundled_selection = service.list().unwrap().skills()[0].selection();
    let workspace_selection = service
        .list_workspace("workspace", workspace.path())
        .unwrap()
        .skills()[0]
        .selection();

    let activated = service
        .activate_workspace(
            "workspace",
            workspace.path(),
            &[workspace_selection, bundled_selection],
        )
        .unwrap();

    assert_eq!(
        activated
            .skills()
            .iter()
            .map(ResolvedSkillPackage::source_kind)
            .collect::<Vec<_>>(),
        vec![SkillSourceKind::Workspace, SkillSourceKind::Bundled]
    );
}

#[test]
fn request_scoped_workspace_activation_rejects_an_ambiguous_registered_source() {
    let workspace = tempdir().unwrap();
    write_workspace_skill(workspace.path(), "workspace-skill");
    let service = SkillsService::for_workspace("workspace", workspace.path()).unwrap();
    let selection = service.list().unwrap().skills()[0].selection();

    let error = service
        .activate_workspace("workspace", workspace.path(), &[selection])
        .unwrap_err();

    assert_eq!(error.code(), SkillErrorCode::DuplicateSource);
    assert!(matches!(
        error,
        SkillActivationError::SourceRegistration {
            source: SkillRegistrationError::DuplicateSource { .. }
        }
    ));
}

#[test]
fn source_identity_requires_non_empty_kind_and_authority() {
    assert!(SkillSourceId::parse(":authority").is_err());
    assert!(SkillSourceId::parse("kind:").is_err());
    assert!(SkillSourceId::parse("kind").is_err());
    assert!(SkillSourceId::parse("kind:authority:").is_err());
    assert!(SkillSourceId::parse("kind::authority").is_err());
    assert!(SkillSourceId::parse("kind:authority").is_ok());
}

#[test]
fn workspace_not_directory_requires_source_reconfiguration() {
    let error = SkillResolveError::Workspace(SkillDiscoveryError::WorkspaceNotDirectory {
        path: PathBuf::from("/not-a-workspace"),
    });

    assert_eq!(error.code(), SkillErrorCode::WorkspaceNotDirectory);
    assert_eq!(error.recovery(), SkillRecovery::ReconfigureSource);
    assert!(!error.is_retryable());

    let fixture = tempdir().unwrap();
    let workspace_file = fixture.path().join("workspace-file");
    fs::write(&workspace_file, "not a directory").unwrap();
    let selection = SkillSelection::parse("workspace:workspace:reviewer", "revision").unwrap();
    let activation_error = SkillsService::new()
        .activate_workspace("workspace", &workspace_file, &[selection])
        .unwrap_err();
    assert_eq!(
        activation_error.code(),
        SkillErrorCode::WorkspaceNotDirectory
    );
    assert_eq!(
        activation_error.recovery(),
        SkillRecovery::ReconfigureSource
    );
}

#[test]
fn activation_preserves_selection_order_and_has_a_stable_revision() {
    let source = Arc::new(MockSource::new(
        "test",
        &[
            ("alpha", "alpha", "# Alpha\n"),
            ("beta", "beta", "# Beta\n"),
        ],
    ));
    let service = SkillsService::from_sources([source as Arc<dyn SkillSource>]).unwrap();
    let catalog = service.list().unwrap();
    let alpha = catalog
        .skills()
        .iter()
        .find(|skill| skill.name() == "alpha")
        .unwrap()
        .selection();
    let beta = catalog
        .skills()
        .iter()
        .find(|skill| skill.name() == "beta")
        .unwrap()
        .selection();

    let selections = [beta.clone(), alpha.clone()];
    let first = service.activate(&selections).unwrap();
    let second = service.activate(&selections).unwrap();
    let reversed = service.activate(&[alpha, beta]).unwrap();

    assert_eq!(
        first
            .skills()
            .iter()
            .map(|skill| skill.name())
            .collect::<Vec<_>>(),
        vec!["beta", "alpha"]
    );
    assert_eq!(first.revision(), second.revision());
    assert_ne!(first.revision(), reversed.revision());
    assert_eq!(
        first.total_source_bytes(),
        first
            .skills()
            .iter()
            .map(|skill| skill.source_text().len())
            .sum::<usize>()
    );
}

#[test]
fn activation_rejects_duplicate_ids_even_when_revisions_match() {
    let source = Arc::new(MockSource::new("test", &[("alpha", "alpha", "# Alpha\n")]));
    let service = SkillsService::from_sources([source as Arc<dyn SkillSource>]).unwrap();
    let selection = service.list().unwrap().skills()[0].selection();

    let error = service
        .activate(&[selection.clone(), selection])
        .unwrap_err();

    assert_eq!(error.code(), SkillErrorCode::DuplicateSelection);
    assert_eq!(error.selection_index(), Some(1));
    assert_eq!(
        error.recovery(),
        super::super::model::SkillRecovery::ChangeSelection
    );
}

#[test]
fn activation_is_all_or_nothing_and_reports_the_original_index() {
    let source = Arc::new(MockSource::new(
        "test",
        &[
            ("alpha", "alpha", "# Alpha\n"),
            ("beta", "beta", "# Beta\n"),
        ],
    ));
    let service = SkillsService::from_sources([source as Arc<dyn SkillSource>]).unwrap();
    let catalog = service.list().unwrap();
    let good = catalog.skills()[0].selection();
    let stale = SkillSelection::new(
        catalog.skills()[1].id().clone(),
        SkillRevision::parse("stale-revision").unwrap(),
    );

    let error = service.activate(&[good, stale]).unwrap_err();

    assert_eq!(error.code(), SkillErrorCode::Stale);
    assert_eq!(error.selection_index(), Some(1));
    assert!(error.actual_revision().is_some());
    assert!(error
        .source_error()
        .is_some_and(SkillResolveError::is_stale));
}

#[test]
fn activation_enforces_count_and_aggregate_source_byte_budgets() {
    let source = Arc::new(MockSource::new(
        "test",
        &[
            ("alpha", "alpha", "# Alpha\n"),
            ("beta", "beta", "# Beta\n"),
        ],
    ));
    let service = SkillsService::from_sources([source as Arc<dyn SkillSource>]).unwrap();
    let selections = service
        .list()
        .unwrap()
        .skills()
        .iter()
        .map(SkillDescriptor::selection)
        .collect::<Vec<_>>();

    let count_limited = service
        .clone_for_test(SkillActivationPolicy::new(1, usize::MAX))
        .activate(&selections)
        .unwrap_err();
    assert_eq!(count_limited.code(), SkillErrorCode::TooManySkills);

    let byte_limited = service
        .clone_for_test(SkillActivationPolicy::new(2, 1))
        .activate(&selections)
        .unwrap_err();
    assert_eq!(byte_limited.code(), SkillErrorCode::SourceBudgetExceeded);
    assert_eq!(
        byte_limited.recovery(),
        super::super::model::SkillRecovery::ReduceSelection
    );
}

#[test]
fn oversized_workspace_identity_is_rejected_before_catalog_generation() {
    let workspace = tempdir().unwrap();
    let workspace_id = "w".repeat(16 * 1024);

    let error = SkillsService::new()
        .list_workspace(&workspace_id, workspace.path())
        .unwrap_err();

    assert_eq!(error.code(), SkillErrorCode::InvalidSource);
    assert_eq!(
        error.recovery(),
        super::super::model::SkillRecovery::ReconfigureSource
    );
}

impl SkillsService {
    fn clone_for_test(&self, policy: SkillActivationPolicy) -> Self {
        Self {
            registry: self.registry.clone(),
            activation_policy: policy,
        }
    }
}
