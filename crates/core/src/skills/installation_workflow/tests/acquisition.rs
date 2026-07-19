use super::*;

#[test]
fn resolved_candidate_moves_exact_bytes_once_into_the_installation_transaction() {
    let fixture = tempdir().unwrap();
    let store = fixture.path().join("store");
    let (workflow, resolution_id, candidate_id, calls) = resolved_handoff(&store);
    let preparation_id = SkillPreparationId::new();
    let request = SkillInstallationPreparationRequest::install(
        preparation_id.clone(),
        installation_id(),
        SkillAcquisitionSource::resolved_candidate(resolution_id.clone(), candidate_id.clone()),
    );

    let preview = workflow.inspect(&request).unwrap();
    assert_eq!(workflow.inspect(&request).unwrap(), preview);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(preview.acquisition().provider(), "github");
    let origin: serde_json::Value =
        serde_json::from_str(preview.acquisition().reference()).unwrap();
    assert_eq!(origin["resolvedCommit"], HANDOFF_COMMIT);

    let consumed = SkillInstallationPreparationRequest::install(
        SkillPreparationId::new(),
        SkillInstallationId::new(),
        SkillAcquisitionSource::resolved_candidate(resolution_id, candidate_id),
    );
    assert!(matches!(
        workflow.inspect(&consumed).unwrap_err(),
        SkillInstallationWorkflowError::SourceResolutionConsumed { .. }
    ));

    workflow
        .commit(&SkillInstallationCommitRequest::new(
            preparation_id,
            preview.preview_revision().clone(),
        ))
        .unwrap();
    let record = SkillInstallationService::new(&store)
        .unwrap()
        .read_installed_skill(&installation_id())
        .unwrap()
        .unwrap();
    assert_eq!(record.provenance().authority().provider(), "github");
    assert!(record.provenance().refresh().is_some());
    let reader = SkillsService::new().with_installed_source(&store).unwrap();
    let catalog = reader.list().unwrap();
    let activated = reader.activate(&[catalog.skills()[0].selection()]).unwrap();
    assert!(activated.skills()[0]
        .instructions()
        .contains("RESOLVED_ONCE_EXACT_BYTES"));
}

#[test]
fn consuming_a_resolved_candidate_immediately_releases_active_resolution_capacity() {
    let fixture = tempdir().unwrap();
    let config = SkillInstallationSessionConfig::new(
        1,
        2,
        crate::skills::installation_session::DEFAULT_SKILL_SESSION_SNAPSHOT_BYTES,
        Duration::from_secs(60),
    )
    .unwrap()
    .with_max_resolution_tombstones(4)
    .unwrap();
    let sessions = SkillInstallationSessionStore::new(config);
    let calls = Arc::new(AtomicUsize::new(0));
    let mut resolutions = SkillSourceResolutionService::with_session_store(sessions.clone());
    resolutions
        .register_resolver(Arc::new(HandoffResolver {
            calls: Arc::clone(&calls),
        }))
        .unwrap();
    let locator = SkillInstallationSourceLocator::url("https://github.com/example/skills").unwrap();
    let first_id = SkillSourceResolutionId::new();
    let first = resolutions
        .resolve_registered(first_id.clone(), &locator)
        .unwrap();
    let candidate_id =
        SkillSourceCandidateId::parse(first.resolution().candidates()[0].candidate_id()).unwrap();
    let workflow = SkillInstallationWorkflow::with_session_store(
        SkillInstallationService::new(fixture.path().join("store")).unwrap(),
        SkillInstallationWorkflowConfig::default(),
        sessions,
    );
    workflow
        .inspect(&SkillInstallationPreparationRequest::install(
            SkillPreparationId::new(),
            installation_id(),
            SkillAcquisitionSource::resolved_candidate(first_id, candidate_id),
        ))
        .unwrap();

    resolutions
        .resolve_registered(SkillSourceResolutionId::new(), &locator)
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn missing_candidate_does_not_consume_the_resolution() {
    let fixture = tempdir().unwrap();
    let (workflow, resolution_id, candidate_id, calls) =
        resolved_handoff(&fixture.path().join("store"));
    let missing = SkillInstallationPreparationRequest::install(
        SkillPreparationId::new(),
        installation_id(),
        SkillAcquisitionSource::resolved_candidate(
            resolution_id.clone(),
            SkillSourceCandidateId::parse("missing").unwrap(),
        ),
    );
    assert!(matches!(
        workflow.inspect(&missing).unwrap_err(),
        SkillInstallationWorkflowError::SourceCandidateNotFound { .. }
    ));

    let selected = SkillInstallationPreparationRequest::install(
        SkillPreparationId::new(),
        installation_id(),
        SkillAcquisitionSource::resolved_candidate(resolution_id, candidate_id),
    );
    workflow.inspect(&selected).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn installed_source_presentation_cannot_change_provider_or_escalate_refreshability() {
    let fixture = tempdir().unwrap();
    let cross_provider_provenance = provider_acquisition(
        "CURRENT",
        PRESENTATION_FIXTURE_PROVIDER,
        "authority",
        Some((PRESENTATION_FIXTURE_PROVIDER, 1, "tracking")),
    )
    .provenance()
    .clone();
    let mut cross_provider_workflow = workflow(&fixture.path().join("cross-provider"));
    cross_provider_workflow
        .register_adapter(Arc::new(PresentationFixtureAdapter {
            presentation: InstalledSkillSourcePresentation::Provider {
                provider: "different-provider".to_string(),
                display_name: "Forged provider".to_string(),
                refreshable: false,
            },
        }))
        .unwrap();
    assert!(matches!(
        cross_provider_workflow
            .installed_source_presentation(&cross_provider_provenance),
        InstalledSkillSourcePresentation::Unknown { ref provider, .. }
            if provider == PRESENTATION_FIXTURE_PROVIDER
    ));

    let non_refreshable_provenance =
        provider_acquisition("CURRENT", PRESENTATION_FIXTURE_PROVIDER, "authority", None)
            .provenance()
            .clone();
    let mut escalating_workflow = workflow(&fixture.path().join("refresh-escalation"));
    escalating_workflow
        .register_adapter(Arc::new(PresentationFixtureAdapter {
            presentation: InstalledSkillSourcePresentation::Provider {
                provider: PRESENTATION_FIXTURE_PROVIDER.to_string(),
                display_name: "Forged refresh capability".to_string(),
                refreshable: true,
            },
        }))
        .unwrap();
    assert!(matches!(
        escalating_workflow.installed_source_presentation(&non_refreshable_provenance),
        InstalledSkillSourcePresentation::Unknown { ref provider, .. }
            if provider == PRESENTATION_FIXTURE_PROVIDER
    ));
    assert!(!escalating_workflow.can_refresh(&non_refreshable_provenance));
}

#[test]
fn installed_source_presentation_rejects_unsafe_provider_display_names() {
    let fixture = tempdir().unwrap();
    let provenance =
        provider_acquisition("CURRENT", PRESENTATION_FIXTURE_PROVIDER, "authority", None)
            .provenance()
            .clone();
    let invalid_display_names = [
        String::new(),
        "   ".to_string(),
        "Fixture\nsource".to_string(),
        "x".repeat(MAX_INSTALLED_SOURCE_DISPLAY_NAME_BYTES + 1),
    ];

    for (index, display_name) in invalid_display_names.into_iter().enumerate() {
        let mut workflow = workflow(&fixture.path().join(format!("invalid-{index}")));
        workflow
            .register_adapter(Arc::new(PresentationFixtureAdapter {
                presentation: InstalledSkillSourcePresentation::Provider {
                    provider: PRESENTATION_FIXTURE_PROVIDER.to_string(),
                    display_name,
                    refreshable: false,
                },
            }))
            .unwrap();

        assert!(matches!(
            workflow.installed_source_presentation(&provenance),
            InstalledSkillSourcePresentation::Unknown { ref provider, .. }
                if provider == PRESENTATION_FIXTURE_PROVIDER
        ));
    }
}

#[test]
fn installed_source_refresh_updates_bytes_and_persists_new_provenance() {
    let fixture = tempdir().unwrap();
    let store = fixture.path().join("store");
    let initial = fixture_acquisition(
        "VERSION_ONE",
        "authority-one",
        Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
    );
    let next = fixture_acquisition(
        "VERSION_TWO",
        "authority-two",
        Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
    );
    let expected_next = next.clone();
    let (workflow, record, calls) = refresh_workflow(&store, initial, next);
    assert!(workflow.can_refresh(record.provenance()));
    assert!(matches!(
        workflow.installed_source_presentation(record.provenance()),
        InstalledSkillSourcePresentation::Provider {
            refreshable: true,
            ..
        }
    ));
    let request = SkillInstallationPreparationRequest::update(
        SkillPreparationId::new(),
        record.skill_id().clone(),
        record.installation_revision().clone(),
        SkillAcquisitionSource::installed_source(),
    );

    let preview = workflow.inspect(&request).unwrap();
    assert!(preview.content_changed());
    assert!(preview.source_changed());
    assert_eq!(workflow.inspect(&request).unwrap(), preview);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    workflow
        .commit(&SkillInstallationCommitRequest::new(
            request.preparation_id().clone(),
            preview.preview_revision().clone(),
        ))
        .unwrap();

    let updated = SkillInstallationService::new(&store)
        .unwrap()
        .read_installed_skill(&installation_id())
        .unwrap()
        .unwrap();
    assert_eq!(
        updated.package_revision(),
        expected_next.package().revision()
    );
    assert_eq!(updated.provenance(), expected_next.provenance());
    assert_ne!(
        updated.installation_revision(),
        record.installation_revision()
    );
}

#[test]
fn installed_source_can_commit_a_provenance_only_update() {
    let fixture = tempdir().unwrap();
    let store = fixture.path().join("store");
    let initial = fixture_acquisition(
        "SAME_BYTES",
        "authority-one",
        Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
    );
    let next = fixture_acquisition(
        "SAME_BYTES",
        "authority-two",
        Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
    );
    let (workflow, record, _) = refresh_workflow(&store, initial, next);
    let request = SkillInstallationPreparationRequest::update(
        SkillPreparationId::new(),
        record.skill_id().clone(),
        record.installation_revision().clone(),
        SkillAcquisitionSource::installed_source(),
    );

    let preview = workflow.inspect(&request).unwrap();
    assert!(!preview.content_changed());
    assert!(preview.source_changed());
    let committed = workflow
        .commit(&SkillInstallationCommitRequest::new(
            request.preparation_id().clone(),
            preview.preview_revision().clone(),
        ))
        .unwrap();
    assert_eq!(
        committed.mutation().outcome(),
        SkillInstallationOutcome::Updated
    );
}

#[test]
fn installed_source_refresh_rejects_cross_provider_adapter_output() {
    let fixture = tempdir().unwrap();
    let store = fixture.path().join("store");
    let initial = fixture_acquisition(
        "VERSION_ONE",
        "authority-one",
        Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
    );
    let next = acquisition_with_provider_parts(
        "VERSION_TWO",
        REFRESH_FIXTURE_PROVIDER,
        "other-provider",
        Some("other-provider"),
    );
    let (workflow, record, calls) = refresh_workflow(&store, initial, next);
    let request = SkillInstallationPreparationRequest::update(
        SkillPreparationId::new(),
        record.skill_id().clone(),
        record.installation_revision().clone(),
        SkillAcquisitionSource::installed_source(),
    );

    let error = workflow.inspect(&request).unwrap_err();
    match error {
        SkillInstallationWorkflowError::Acquisition { provider, source } => {
            assert_eq!(provider.as_str(), REFRESH_FIXTURE_PROVIDER);
            assert_eq!(source.code(), SkillAcquisitionAdapterErrorCode::Unavailable);
            assert!(!source.reason().contains("other-provider"));
        }
        other => panic!("unexpected provider-binding error: {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(SkillInstallationService::new(&store)
        .unwrap()
        .read_installed_skill(&installation_id())
        .unwrap()
        .is_some_and(
            |installed| installed.installation_revision() == record.installation_revision()
        ));
}

#[test]
fn stale_installed_source_preflight_never_calls_the_adapter() {
    let fixture = tempdir().unwrap();
    let initial = fixture_acquisition(
        "VERSION_ONE",
        "authority-one",
        Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
    );
    let next = fixture_acquisition(
        "VERSION_TWO",
        "authority-two",
        Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
    );
    let (workflow, record, calls) = refresh_workflow(&fixture.path().join("store"), initial, next);
    let stale = SkillInstallationRevision::parse(format!(
        "skill-installation-sha256-v1:{}",
        "0".repeat(64)
    ))
    .unwrap();
    assert_ne!(&stale, record.installation_revision());
    let request = SkillInstallationPreparationRequest::update(
        SkillPreparationId::new(),
        record.skill_id().clone(),
        stale,
        SkillAcquisitionSource::installed_source(),
    );

    assert!(matches!(
        workflow.inspect(&request).unwrap_err(),
        SkillInstallationWorkflowError::InstalledSourceRevisionConflict { .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn installed_source_rejects_missing_provider_unsupported_schema_and_install_intent() {
    let fixture = tempdir().unwrap();
    let next = fixture_acquisition(
        "NEXT",
        "authority-two",
        Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
    );

    let unknown_initial = provider_acquisition(
        "CURRENT",
        "missing-provider",
        "authority-one",
        Some(("missing-provider", 1, "tracking")),
    );
    let (unknown_workflow, unknown_record, unknown_calls) = refresh_workflow(
        &fixture.path().join("unknown"),
        unknown_initial,
        next.clone(),
    );
    let unknown_request = SkillInstallationPreparationRequest::update(
        SkillPreparationId::new(),
        unknown_record.skill_id().clone(),
        unknown_record.installation_revision().clone(),
        SkillAcquisitionSource::installed_source(),
    );
    assert!(matches!(
        unknown_workflow.inspect(&unknown_request).unwrap_err(),
        SkillInstallationWorkflowError::UnknownRefreshProvider { .. }
    ));
    assert_eq!(unknown_calls.load(Ordering::SeqCst), 0);

    let delegated_initial = fixture_acquisition(
        "CURRENT",
        "authority-one",
        Some(("missing-provider", 1, "tracking")),
    );
    let (delegated_workflow, delegated_record, delegated_calls) = refresh_workflow(
        &fixture.path().join("delegated"),
        delegated_initial,
        next.clone(),
    );
    let delegated_request = SkillInstallationPreparationRequest::update(
        SkillPreparationId::new(),
        delegated_record.skill_id().clone(),
        delegated_record.installation_revision().clone(),
        SkillAcquisitionSource::installed_source(),
    );
    assert!(!delegated_workflow.can_refresh(delegated_record.provenance()));
    assert!(matches!(
        delegated_workflow.installed_source_presentation(delegated_record.provenance()),
        InstalledSkillSourcePresentation::Unknown { .. }
    ));
    assert!(matches!(
        delegated_workflow.inspect(&delegated_request).unwrap_err(),
        SkillInstallationWorkflowError::InvalidInstalledSourceProvenance { .. }
    ));
    assert_eq!(delegated_calls.load(Ordering::SeqCst), 0);

    let schema_initial = fixture_acquisition(
        "CURRENT",
        "authority-one",
        Some((REFRESH_FIXTURE_PROVIDER, 2, "tracking")),
    );
    let (schema_workflow, schema_record, schema_calls) =
        refresh_workflow(&fixture.path().join("schema"), schema_initial, next);
    let schema_request = SkillInstallationPreparationRequest::update(
        SkillPreparationId::new(),
        schema_record.skill_id().clone(),
        schema_record.installation_revision().clone(),
        SkillAcquisitionSource::installed_source(),
    );
    assert!(matches!(
        schema_workflow.inspect(&schema_request).unwrap_err(),
        SkillInstallationWorkflowError::UnsupportedRefreshSchema { .. }
    ));
    assert_eq!(schema_calls.load(Ordering::SeqCst), 0);

    let install_request = SkillInstallationPreparationRequest::install(
        SkillPreparationId::new(),
        SkillInstallationId::new(),
        SkillAcquisitionSource::installed_source(),
    );
    assert!(matches!(
        schema_workflow.inspect(&install_request).unwrap_err(),
        SkillInstallationWorkflowError::InstalledSourceRequiresUpdate
    ));
}

#[test]
fn local_installation_is_presented_as_nonrefreshable_without_paths() {
    let fixture = tempdir().unwrap();
    let source = fixture.path().join("source");
    let store = fixture.path().join("store");
    write_skill(&source, "LOCAL", false);
    let workflow = workflow(&store);
    let preview = workflow
        .inspect_local_directory_install(SkillPreparationId::new(), installation_id(), &source)
        .unwrap();
    workflow
        .commit(&SkillInstallationCommitRequest::new(
            preview.preparation_id().clone(),
            preview.preview_revision().clone(),
        ))
        .unwrap();
    let record = SkillInstallationService::new(&store)
        .unwrap()
        .read_installed_skill(&installation_id())
        .unwrap()
        .unwrap();
    assert!(!workflow.can_refresh(record.provenance()));
    assert_eq!(
        workflow.installed_source_presentation(record.provenance()),
        InstalledSkillSourcePresentation::LocalDirectory
    );
    assert!(!record
        .provenance()
        .authority()
        .payload()
        .contains(source.to_str().unwrap()));

    let request = SkillInstallationPreparationRequest::update(
        SkillPreparationId::new(),
        record.skill_id().clone(),
        record.installation_revision().clone(),
        SkillAcquisitionSource::installed_source(),
    );
    assert!(matches!(
        workflow.inspect(&request).unwrap_err(),
        SkillInstallationWorkflowError::InstalledSourceNotRefreshable { .. }
    ));
}
