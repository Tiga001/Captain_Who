use super::*;

#[test]
fn adapter_panic_releases_the_preparation_for_an_idempotent_retry() {
    const PROVIDER: &str = "panic-once-adapter";

    struct PanicOnceAdapter {
        calls: AtomicUsize,
        next: PreparedSkillAcquisition,
    }

    impl SkillAcquisitionAdapter for PanicOnceAdapter {
        fn provider(&self) -> SkillAcquisitionProvider {
            SkillAcquisitionProvider::parse(PROVIDER).unwrap()
        }

        fn acquire(
            &self,
            _source: &SkillAcquisitionSource,
        ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("injected adapter panic");
            }
            Ok(self.next.clone())
        }
    }

    let fixture = tempdir().unwrap();
    let provider = SkillAcquisitionProvider::parse(PROVIDER).unwrap();
    let mut workflow = workflow(&fixture.path().join("store"));
    workflow
        .register_adapter(Arc::new(PanicOnceAdapter {
            calls: AtomicUsize::new(0),
            next: provider_acquisition("RECOVERED", PROVIDER, "authority", None),
        }))
        .unwrap();
    let request = SkillInstallationPreparationRequest::install(
        SkillPreparationId::new(),
        installation_id(),
        SkillAcquisitionSource::adapter(provider, b"fixture".to_vec()).unwrap(),
    );

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = workflow.inspect(&request);
    }));
    assert!(panic.is_err());
    let preview = workflow
        .inspect(&request)
        .expect("the same preparation must be retryable after an adapter panic");
    assert_eq!(preview.package().name(), "refresh-fixture");
}

#[test]
fn stale_preparing_recovery_cannot_remove_a_later_attempt() {
    let fixture = tempdir().unwrap();
    let workflow = workflow(&fixture.path().join("store"));
    let preparation_id = SkillPreparationId::new();
    let request = SkillInstallationPreparationRequest::install(
        preparation_id.clone(),
        installation_id(),
        SkillAcquisitionSource::local_directory(fixture.path().join("skill")),
    );

    let first_attempt = {
        let now = workflow.sessions.now();
        let mut state = workflow.sessions.lock().unwrap();
        workflow
            .reserve_preparation(&mut state, request.clone(), now.monotonic)
            .unwrap()
    };
    let stale_recovery =
        PreparingSlotRecovery::new(&workflow.sessions, &preparation_id, first_attempt);

    let second_attempt = {
        let now = workflow.sessions.now();
        let mut state = workflow.sessions.lock().unwrap();
        state.preparations.remove(&preparation_id);
        workflow
            .reserve_preparation(&mut state, request, now.monotonic)
            .unwrap()
    };
    assert_ne!(first_attempt, second_attempt);

    drop(stale_recovery);

    let state = workflow.sessions.lock().unwrap();
    assert!(matches!(
        state.preparations.get(&preparation_id),
        Some(PreparationSlot::Preparing { attempt_id, .. })
            if *attempt_id == second_attempt
    ));
}

#[test]
fn commit_panic_recovery_restores_the_exact_ready_snapshot() {
    let fixture = tempdir().unwrap();
    let directory = fixture.path().join("skill");
    write_skill(&directory, "RECOVER_COMMIT", false);
    let workflow = workflow(&fixture.path().join("store"));
    let preparation_id = SkillPreparationId::new();
    let preparation = SkillInstallationPreparationRequest::install(
        preparation_id.clone(),
        installation_id(),
        SkillAcquisitionSource::local_directory(directory),
    );
    let preview = workflow.inspect(&preparation).unwrap();

    let (stored_request, acquisition, snapshot_bytes, expires_at, attempt_id) = {
        let mut state = workflow.sessions.lock().unwrap();
        let slot = state.preparations.remove(&preparation_id).unwrap();
        let PreparationSlot::Ready {
            request,
            preview: stored_preview,
            acquisition,
            snapshot_bytes,
            expires_at,
        } = slot
        else {
            panic!("inspection must leave a ready preparation");
        };
        assert_eq!(stored_preview, preview);
        let attempt_id = state.next_preparation_attempt();
        state.preparations.insert(
            preparation_id.clone(),
            PreparationSlot::Committing {
                request: request.clone(),
                attempt_id,
                snapshot_bytes,
            },
        );
        (request, acquisition, snapshot_bytes, expires_at, attempt_id)
    };

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _recovery = CommittingSlotRecovery::new(
            &workflow.sessions,
            attempt_id,
            &stored_request,
            &preview,
            &acquisition,
            snapshot_bytes,
            expires_at,
        );
        panic!("injected commit panic");
    }));
    assert!(panic.is_err());

    let committed = workflow
        .commit(&SkillInstallationCommitRequest::new(
            preparation_id,
            preview.preview_revision().clone(),
        ))
        .expect("commit retry must consume the restored exact snapshot");
    assert_eq!(committed.preview(), &preview);
}

#[test]
fn stale_committing_recovery_cannot_overwrite_a_later_attempt() {
    let fixture = tempdir().unwrap();
    let directory = fixture.path().join("skill");
    write_skill(&directory, "STALE_COMMIT_GUARD", false);
    let workflow = workflow(&fixture.path().join("store"));
    let preparation_id = SkillPreparationId::new();
    let preparation = SkillInstallationPreparationRequest::install(
        preparation_id.clone(),
        installation_id(),
        SkillAcquisitionSource::local_directory(directory),
    );
    let preview = workflow.inspect(&preparation).unwrap();

    let (stored_request, acquisition, snapshot_bytes, expires_at, first_attempt) = {
        let mut state = workflow.sessions.lock().unwrap();
        let slot = state.preparations.remove(&preparation_id).unwrap();
        let PreparationSlot::Ready {
            request,
            acquisition,
            snapshot_bytes,
            expires_at,
            ..
        } = slot
        else {
            panic!("inspection must leave a ready preparation");
        };
        let attempt_id = state.next_preparation_attempt();
        state.preparations.insert(
            preparation_id.clone(),
            PreparationSlot::Committing {
                request: request.clone(),
                attempt_id,
                snapshot_bytes,
            },
        );
        (request, acquisition, snapshot_bytes, expires_at, attempt_id)
    };
    let stale_recovery = CommittingSlotRecovery::new(
        &workflow.sessions,
        first_attempt,
        &stored_request,
        &preview,
        &acquisition,
        snapshot_bytes,
        expires_at,
    );

    let second_attempt = {
        let mut state = workflow.sessions.lock().unwrap();
        let attempt_id = state.next_preparation_attempt();
        state.preparations.insert(
            preparation_id.clone(),
            PreparationSlot::Committing {
                request: stored_request,
                attempt_id,
                snapshot_bytes,
            },
        );
        attempt_id
    };
    assert_ne!(first_attempt, second_attempt);

    drop(stale_recovery);

    let state = workflow.sessions.lock().unwrap();
    assert!(matches!(
        state.preparations.get(&preparation_id),
        Some(PreparationSlot::Committing { attempt_id, .. })
            if *attempt_id == second_attempt
    ));
}

#[test]
fn registered_adapter_cannot_cross_its_provider_ownership_boundary() {
    let mismatched = [
        acquisition_with_provider_parts(
            "ORIGIN_MISMATCH",
            "other-provider",
            REFRESH_FIXTURE_PROVIDER,
            None,
        ),
        acquisition_with_provider_parts(
            "AUTHORITY_MISMATCH",
            REFRESH_FIXTURE_PROVIDER,
            "other-provider",
            None,
        ),
        acquisition_with_provider_parts(
            "REFRESH_MISMATCH",
            REFRESH_FIXTURE_PROVIDER,
            REFRESH_FIXTURE_PROVIDER,
            Some("other-provider"),
        ),
    ];

    for acquisition in mismatched {
        let fixture = tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut workflow = workflow(&fixture.path().join("store"));
        workflow
            .register_adapter(Arc::new(RefreshFixtureAdapter {
                calls: Arc::clone(&calls),
                next: acquisition,
            }))
            .unwrap();
        let provider = SkillAcquisitionProvider::parse(REFRESH_FIXTURE_PROVIDER).unwrap();
        let request = SkillInstallationPreparationRequest::install(
            SkillPreparationId::new(),
            installation_id(),
            SkillAcquisitionSource::adapter(provider.clone(), b"fixture".to_vec()).unwrap(),
        );

        let error = workflow.inspect(&request).unwrap_err();
        match error {
            SkillInstallationWorkflowError::Acquisition {
                provider: actual,
                source,
            } => {
                assert_eq!(actual, provider);
                assert_eq!(source.code(), SkillAcquisitionAdapterErrorCode::Unavailable);
                assert!(!source.reason().contains("other-provider"));
            }
            other => panic!("unexpected provider-binding error: {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn retry_returns_the_same_absolute_expiration_and_preview_revision() {
    let fixture = tempdir().unwrap();
    let source = fixture.path().join("source");
    write_skill(&source, "STABLE_PREVIEW", false);
    let config =
        SkillInstallationWorkflowConfig::new(2, MAX_SKILL_PACKAGE_BYTES, Duration::from_secs(10))
            .unwrap();
    let clock = Arc::new(ManualClock::default());
    let workflow = SkillInstallationWorkflow::with_clock(
        SkillInstallationService::new(fixture.path().join("store")).unwrap(),
        config,
        clock.clone(),
    );
    let request = SkillInstallationPreparationRequest::install(
        SkillPreparationId::new(),
        installation_id(),
        SkillAcquisitionSource::local_directory(&source),
    );

    let first = workflow.inspect(&request).unwrap();
    assert_eq!(first.expires_at_unix_ms(), 1_700_000_010_000);
    clock.advance(Duration::from_secs(5));
    let retry = workflow.inspect(&request).unwrap();
    assert_eq!(retry.expires_at_unix_ms(), first.expires_at_unix_ms());
    assert_eq!(retry.preview_revision(), first.preview_revision());
}

#[test]
fn failed_commit_does_not_extend_the_original_preview_deadline() {
    let fixture = tempdir().unwrap();
    let store = fixture.path().join("store");
    let source = fixture.path().join("source");
    let conflicting = fixture.path().join("conflicting");
    write_skill(&source, "PREPARED_TARGET", false);
    write_skill(&conflicting, "OTHER_PACKAGE", false);
    let config =
        SkillInstallationWorkflowConfig::new(2, MAX_SKILL_PACKAGE_BYTES, Duration::from_secs(10))
            .unwrap();
    let clock = Arc::new(ManualClock::default());
    let workflow = SkillInstallationWorkflow::with_clock(
        SkillInstallationService::new(&store).unwrap(),
        config,
        clock.clone(),
    );
    let preview = workflow
        .inspect_local_directory_install(SkillPreparationId::new(), installation_id(), &source)
        .unwrap();

    let competing_package =
        PreparedSkillPackage::from_local_directory(&conflicting, "competing-test").unwrap();
    SkillInstallationService::new(&store)
        .unwrap()
        .install_prepared(installation_id(), competing_package)
        .unwrap();
    clock.advance(Duration::from_secs(9));
    let commit = SkillInstallationCommitRequest::new(
        preview.preparation_id().clone(),
        preview.preview_revision().clone(),
    );
    assert!(matches!(
        workflow.commit(&commit).unwrap_err(),
        SkillInstallationWorkflowError::Installation { .. }
    ));

    clock.advance(Duration::from_secs(2));
    assert!(matches!(
        workflow.commit(&commit).unwrap_err(),
        SkillInstallationWorkflowError::PreparationNotFoundOrExpired { .. }
    ));
}

#[test]
fn expiration_and_cancellation_release_snapshot_capacity() {
    let fixture = tempdir().unwrap();
    let first_source = fixture.path().join("first");
    let second_source = fixture.path().join("second");
    write_skill(&first_source, "FIRST", false);
    write_skill(&second_source, "SECOND", false);
    let config =
        SkillInstallationWorkflowConfig::new(1, MAX_SKILL_PACKAGE_BYTES, Duration::from_secs(10))
            .unwrap();
    let clock = Arc::new(ManualClock::default());
    let workflow = SkillInstallationWorkflow::with_clock(
        SkillInstallationService::new(fixture.path().join("store")).unwrap(),
        config,
        clock.clone(),
    );
    let first_id = SkillPreparationId::new();
    let first_preview = workflow
        .inspect_local_directory_install(first_id.clone(), installation_id(), &first_source)
        .unwrap();
    assert!(matches!(
        workflow
            .inspect_local_directory_install(
                SkillPreparationId::new(),
                SkillInstallationId::new(),
                &second_source,
            )
            .unwrap_err(),
        SkillInstallationWorkflowError::PreparationCapacityExceeded { .. }
    ));
    assert_eq!(
        workflow.cancel(&first_id).unwrap(),
        SkillPreparationCancellation::Cancelled
    );
    assert_eq!(
        workflow.cancel(&first_id).unwrap(),
        SkillPreparationCancellation::AlreadyCancelled
    );

    clock.advance(Duration::from_secs(11));
    workflow
        .inspect_local_directory_install(
            SkillPreparationId::new(),
            SkillInstallationId::new(),
            &second_source,
        )
        .unwrap();
    assert!(matches!(
        workflow
            .commit(&SkillInstallationCommitRequest::new(
                first_id,
                first_preview.preview_revision().clone(),
            ))
            .unwrap_err(),
        SkillInstallationWorkflowError::PreparationNotFoundOrExpired { .. }
    ));
}

#[test]
fn debug_representations_do_not_disclose_acquisition_paths_or_payloads() {
    let secret_path = PathBuf::from("/private/secret/skill");
    let source = SkillAcquisitionSource::local_directory(&secret_path);
    let request = SkillInstallationPreparationRequest::install(
        SkillPreparationId::new(),
        installation_id(),
        source,
    );
    let debug = format!("{request:?}");
    assert!(!debug.contains(secret_path.to_str().unwrap()));
    assert!(debug.contains("[redacted]"));

    let provider = SkillAcquisitionProvider::parse("fixture").unwrap();
    let secret = b"https://token@example.invalid/private".to_vec();
    let source = SkillAcquisitionSource::adapter(provider, secret.clone()).unwrap();
    let debug = format!("{source:?}");
    assert!(!debug.contains(&String::from_utf8(secret).unwrap()));
}

#[test]
fn canonical_uuid_and_configuration_contracts_are_strict() {
    assert!(SkillPreparationId::parse("01234567-89ab-4def-8123-456789abcdef").is_ok());
    assert!(SkillPreparationId::parse("01234567-89AB-4DEF-8123-456789ABCDEF").is_err());
    assert!(SkillPreparationId::parse(Uuid::nil().hyphenated().to_string()).is_err());
    assert!(SkillInstallationWorkflowConfig::new(
        0,
        MAX_SKILL_PACKAGE_BYTES,
        Duration::from_secs(1),
    )
    .is_err());
    assert!(
        SkillInstallationWorkflowConfig::new(1, MAX_SKILL_PACKAGE_BYTES, Duration::ZERO,).is_err()
    );
}
