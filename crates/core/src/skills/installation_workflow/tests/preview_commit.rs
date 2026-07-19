use super::*;

#[test]
fn local_preview_is_idempotent_and_commit_uses_the_captured_snapshot() {
    let fixture = tempdir().unwrap();
    let store = fixture.path().join("store");
    let source = fixture.path().join("source");
    write_skill(&source, "ORIGINAL", false);
    let workflow = workflow(&store);
    let preparation_id = SkillPreparationId::new();
    let request = SkillInstallationPreparationRequest::install(
        preparation_id.clone(),
        installation_id(),
        SkillAcquisitionSource::local_directory(&source),
    );

    let first = workflow.inspect(&request).unwrap();
    assert!(first.content_changed());
    assert!(first.source_changed());
    assert_eq!(first.acquisition().provider(), LOCAL_DIRECTORY_PROVIDER);
    assert_eq!(
        first.acquisition().reference(),
        LOCAL_DIRECTORY_ORIGIN_REFERENCE
    );
    assert!(!format!("{first:?}").contains(source.to_str().unwrap()));
    write_skill(&source, "MUTATED", false);
    let retry = workflow.inspect(&request).unwrap();
    assert_eq!(first, retry);

    let committed = workflow
        .commit(&SkillInstallationCommitRequest::new(
            preparation_id.clone(),
            first.preview_revision().clone(),
        ))
        .unwrap();
    assert_eq!(
        committed.mutation().outcome(),
        SkillInstallationOutcome::Installed
    );
    assert_eq!(committed.preview(), &first);
    assert!(!committed.replayed());

    let replay = workflow
        .commit(&SkillInstallationCommitRequest::new(
            preparation_id,
            first.preview_revision().clone(),
        ))
        .unwrap();
    assert!(replay.replayed());
    assert_eq!(replay.preview(), &first);
    assert_eq!(replay.mutation(), committed.mutation());

    let reader = SkillsService::new().with_installed_source(&store).unwrap();
    let catalog = reader.list().unwrap();
    let activated = reader.activate(&[catalog.skills()[0].selection()]).unwrap();
    assert!(activated.skills()[0].instructions().contains("ORIGINAL"));
    assert!(!activated.skills()[0].instructions().contains("MUTATED"));
}

#[test]
fn preview_revision_is_required_for_first_commit_and_committed_replay() {
    let fixture = tempdir().unwrap();
    let store = fixture.path().join("store");
    let source = fixture.path().join("source");
    write_skill(&source, "PREVIEW_BOUND", false);
    let workflow = workflow(&store);
    let preparation_id = SkillPreparationId::new();
    let preview = workflow
        .inspect_local_directory_install(preparation_id.clone(), installation_id(), &source)
        .unwrap();
    let wrong = SkillPreviewRevision::parse(format!("{PREVIEW_REVISION_PREFIX}{}", "0".repeat(64)))
        .unwrap();

    assert!(matches!(
        workflow
            .commit(&SkillInstallationCommitRequest::new(
                preparation_id.clone(),
                wrong.clone(),
            ))
            .unwrap_err(),
        SkillInstallationWorkflowError::PreviewMismatch { .. }
    ));
    let committed = workflow
        .commit(&SkillInstallationCommitRequest::new(
            preparation_id.clone(),
            preview.preview_revision().clone(),
        ))
        .unwrap();
    assert!(!committed.replayed());
    assert!(matches!(
        workflow
            .commit(&SkillInstallationCommitRequest::new(preparation_id, wrong))
            .unwrap_err(),
        SkillInstallationWorkflowError::PreviewMismatch { .. }
    ));
}

#[test]
fn package_and_installation_revisions_keep_distinct_update_contracts() {
    let fixture = tempdir().unwrap();
    let store = fixture.path().join("store");
    let source = fixture.path().join("source");
    write_skill(&source, "VERSION_ONE", false);
    let workflow = workflow(&store);

    let install_preview = workflow
        .inspect_local_directory_install(SkillPreparationId::new(), installation_id(), &source)
        .unwrap();
    let install = workflow
        .commit(&SkillInstallationCommitRequest::new(
            install_preview.preparation_id().clone(),
            install_preview.preview_revision().clone(),
        ))
        .unwrap();
    let installed_package_revision = install.mutation().package_revision().unwrap().clone();
    let installed_revision = install.mutation().installation_revision().unwrap().clone();
    assert_eq!(
        installed_package_revision,
        *install_preview.package().revision()
    );

    write_skill(&source, "VERSION_TWO", false);
    let update_preview = workflow
        .inspect_local_directory_update(
            SkillPreparationId::new(),
            install.mutation().skill_id().clone(),
            installed_revision.clone(),
            &source,
        )
        .unwrap();
    assert!(update_preview.content_changed());
    assert!(!update_preview.source_changed());
    assert_eq!(
        update_preview.expected_revision(),
        Some(&installed_revision)
    );
    assert_ne!(
        update_preview.package().revision(),
        &installed_package_revision
    );
    let update = workflow
        .commit(&SkillInstallationCommitRequest::new(
            update_preview.preparation_id().clone(),
            update_preview.preview_revision().clone(),
        ))
        .unwrap();
    assert_eq!(update.preview(), &update_preview);
    assert_eq!(
        update.preview().operation(),
        SkillInstallationOperation::Update
    );
    let updated_package_revision = update.mutation().package_revision().unwrap().clone();
    let updated_revision = update.mutation().installation_revision().unwrap().clone();
    assert_eq!(
        updated_package_revision,
        *update_preview.package().revision()
    );

    // The lifecycle revision returned by commit is accepted directly by
    // the following exact update CAS.
    write_skill(&source, "VERSION_THREE", false);
    let next_preview = workflow
        .inspect_local_directory_update(
            SkillPreparationId::new(),
            update.mutation().skill_id().clone(),
            updated_revision,
            &source,
        )
        .unwrap();
    workflow
        .commit(&SkillInstallationCommitRequest::new(
            next_preview.preparation_id().clone(),
            next_preview.preview_revision().clone(),
        ))
        .unwrap();
}

#[test]
fn preparation_identity_cannot_be_rebound_to_a_different_request() {
    let fixture = tempdir().unwrap();
    let source = fixture.path().join("source");
    write_skill(&source, "ONE", false);
    let workflow = workflow(&fixture.path().join("store"));
    let preparation_id = SkillPreparationId::new();
    let first = SkillInstallationPreparationRequest::install(
        preparation_id.clone(),
        installation_id(),
        SkillAcquisitionSource::local_directory(&source),
    );
    workflow.inspect(&first).unwrap();

    let different = SkillInstallationPreparationRequest::install(
        preparation_id,
        SkillInstallationId::new(),
        SkillAcquisitionSource::local_directory(&source),
    );
    assert!(matches!(
        workflow.inspect(&different).unwrap_err(),
        SkillInstallationWorkflowError::PreparationConflict { .. }
    ));
}

#[test]
fn resource_summary_and_script_acknowledgement_gate_the_commit() {
    let fixture = tempdir().unwrap();
    let source = fixture.path().join("source");
    let store = fixture.path().join("store");
    write_skill(&source, "RESOURCEFUL", true);
    let workflow = workflow(&store);
    let preparation_id = SkillPreparationId::new();
    let preview = workflow
        .inspect_local_directory_install(preparation_id.clone(), installation_id(), &source)
        .unwrap();

    assert_eq!(preview.package().resources().resource_count(), 3);
    assert_eq!(preview.package().resources().reference_count(), 1);
    assert_eq!(preview.package().resources().asset_count(), 1);
    assert_eq!(preview.package().resources().script_count(), 1);
    assert_eq!(preview.warnings().len(), 2);
    assert_eq!(
        preview.warnings()[0].code(),
        SkillInstallationWarningCode::ResourcesNotExposed
    );
    assert_eq!(
        preview.warnings()[1].code(),
        SkillInstallationWarningCode::ContainsScripts
    );

    let error = workflow
        .commit(&SkillInstallationCommitRequest::new(
            preparation_id.clone(),
            preview.preview_revision().clone(),
        ))
        .unwrap_err();
    assert_eq!(
        error.missing_warning_acknowledgements(),
        Some(&[SkillInstallationWarningCode::ContainsScripts][..])
    );
    assert!(SkillsService::new()
        .with_installed_source(&store)
        .unwrap()
        .list()
        .unwrap()
        .skills()
        .is_empty());

    workflow
        .commit(
            &SkillInstallationCommitRequest::new(
                preparation_id,
                preview.preview_revision().clone(),
            )
            .acknowledge(SkillInstallationWarningCode::ContainsScripts),
        )
        .unwrap();
}

#[test]
fn registered_second_adapter_enters_the_same_transaction() {
    struct FixtureAdapter {
        provider: SkillAcquisitionProvider,
        calls: Arc<AtomicUsize>,
    }

    impl SkillAcquisitionAdapter for FixtureAdapter {
        fn provider(&self) -> SkillAcquisitionProvider {
            self.provider.clone()
        }

        fn acquire(
            &self,
            source: &SkillAcquisitionSource,
        ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let marker = source.adapter_request().ok_or_else(|| {
                SkillAcquisitionAdapterError::invalid_request("missing fixture request")
            })?;
            let origin = SkillPackageOrigin::new("fixture-adapter", "fixture")
                .map_err(|_| SkillAcquisitionAdapterError::unavailable("invalid origin"))?;
            let package = PreparedSkillPackage::from_bytes(
                format!(
                    "---\nname: adapter-fixture\ndescription: Adapter fixture.\n---\n# Instructions\n{}\n",
                    String::from_utf8_lossy(marker)
                )
                .into_bytes(),
                origin,
            )
            .map_err(SkillAcquisitionAdapterError::from)?;
            let authority =
                SkillInstallationAuthority::new("fixture-adapter", 1, "fixture-authority")
                    .map_err(|_| SkillAcquisitionAdapterError::unavailable("invalid provenance"))?;
            Ok(PreparedSkillAcquisition::new(
                package,
                SkillInstallationProvenance::new(authority, None),
            ))
        }
    }

    let fixture = tempdir().unwrap();
    let provider = SkillAcquisitionProvider::parse("fixture-adapter").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut workflow = workflow(&fixture.path().join("store"));
    workflow
        .register_adapter(Arc::new(FixtureAdapter {
            provider: provider.clone(),
            calls: Arc::clone(&calls),
        }))
        .unwrap();
    let preparation_id = SkillPreparationId::new();
    let request = SkillInstallationPreparationRequest::install(
        preparation_id.clone(),
        installation_id(),
        SkillAcquisitionSource::adapter(provider, b"FROM_ADAPTER".to_vec()).unwrap(),
    );

    let first = workflow.inspect(&request).unwrap();
    let second = workflow.inspect(&request).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.acquisition().provider(), "fixture-adapter");
    assert_eq!(first.acquisition().reference(), "fixture");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    workflow
        .commit(&SkillInstallationCommitRequest::new(
            preparation_id,
            first.preview_revision().clone(),
        ))
        .unwrap();
}
