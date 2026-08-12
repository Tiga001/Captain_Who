use super::*;

#[test]
fn local_directory_preparation_installs_and_resolves_exact_instructions() {
    let fixture = tempdir().unwrap();
    let acquisition = fixture.path().join("acquired-skill");
    fs::create_dir(&acquisition).unwrap();
    fs::write(
        acquisition.join(SKILL_FILE_NAME),
        "---\nname: local-auditor\ndescription: Local vertical slice.\n---\n# Instructions\nLOCAL_EXACT_INSTRUCTIONS\n",
    )
    .unwrap();
    let prepared =
        PreparedSkillPackage::from_local_directory(&acquisition, "user-selected-directory")
            .unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let request = install_request(INSTALLATION_ID, prepared);

    installer.install(&request).unwrap();
    let service = SkillsService::new().with_installed_source(&root).unwrap();
    let catalog = service.list().unwrap();
    assert_eq!(catalog.skills().len(), 1);
    let resolved = service.resolve(&catalog.skills()[0].selection()).unwrap();
    assert!(resolved.instructions().contains("LOCAL_EXACT_INSTRUCTIONS"));
    assert_eq!(
        resolved.descriptor().id().local_id(),
        request.installation_id().as_str()
    );
    let receipt = ManagedSkillStore::new(&root)
        .unwrap()
        .load_receipt(request.installation_id())
        .unwrap();
    assert_eq!(
        receipt._origin.provider(),
        crate::skills::LOCAL_DIRECTORY_SKILL_ORIGIN_PROVIDER
    );
    assert_eq!(receipt._origin.reference(), "user-selected-directory");
}

#[test]
fn install_rejects_id_reuse_and_never_repairs_corrupt_state() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let first = install_request(INSTALLATION_ID, package("first", "FIRST"));
    installer.install(&first).unwrap();

    let different = install_request(INSTALLATION_ID, package("second", "SECOND"));
    assert_eq!(
        installer.install(&different).unwrap_err().code(),
        ManagedSkillInstallerErrorCode::InstallationExists
    );

    fs::write(
        root.join(INSTALLATIONS_DIRECTORY)
            .join(format!("{INSTALLATION_ID}.json")),
        b"{not-json",
    )
    .unwrap();
    assert_eq!(
        installer.install(&first).unwrap_err().code(),
        ManagedSkillInstallerErrorCode::StoreCorrupt
    );
    assert_eq!(
        fs::read(
            root.join(INSTALLATIONS_DIRECTORY)
                .join(format!("{INSTALLATION_ID}.json"))
        )
        .unwrap(),
        b"{not-json"
    );
}

#[test]
fn install_failpoints_preserve_absent_or_fully_committed_state_and_retry() {
    let precommit = [
        ManagedSkillInstallerFailpoint::PackageFileSynced,
        ManagedSkillInstallerFailpoint::PackagePublished,
        ManagedSkillInstallerFailpoint::PackageParentSynced,
        ManagedSkillInstallerFailpoint::ReceiptFileSynced,
    ];
    for failpoint in precommit {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let request = install_request(INSTALLATION_ID, package("auditor", "INSTALL"));
        let failing = ManagedSkillInstaller::with_failpoint(&root, failpoint);

        let error = failing.install(&request).unwrap_err();
        assert_eq!(error.code(), ManagedSkillInstallerErrorCode::Io);
        assert!(installed_catalog(&root).skills().is_empty());
        assert!(
            package_staging_entries(&root).is_empty(),
            "{failpoint:?} must not leak package staging directories"
        );
        assert_eq!(
            ManagedSkillInstaller::new(&root)
                .unwrap()
                .install(&request)
                .unwrap(),
            ManagedSkillInstallOutcome::Installed
        );
        assert_eq!(installed_catalog(&root).skills().len(), 1);
    }

    for failpoint in [
        ManagedSkillInstallerFailpoint::ReceiptPublished,
        ManagedSkillInstallerFailpoint::ReceiptParentSynced,
    ] {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let request = install_request(INSTALLATION_ID, package("auditor", "INSTALL"));
        let failing = ManagedSkillInstaller::with_failpoint(&root, failpoint);

        let error = failing.install(&request).unwrap_err();
        assert_eq!(
            error.code(),
            ManagedSkillInstallerErrorCode::CommitIndeterminate
        );
        assert!(error.commit_may_have_succeeded());
        assert_eq!(installed_catalog(&root).skills().len(), 1);
        assert_eq!(
            ManagedSkillInstaller::new(&root)
                .unwrap()
                .install(&request)
                .unwrap(),
            ManagedSkillInstallOutcome::AlreadyInstalled
        );
    }
}

#[test]
fn retry_resyncs_an_orphan_package_before_committing_its_receipt() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let request = install_request(INSTALLATION_ID, package("auditor", "ORPHAN"));

    let publish_failure = ManagedSkillInstaller::with_failpoint(
        &root,
        ManagedSkillInstallerFailpoint::PackagePublished,
    );
    assert_eq!(
        publish_failure.install(&request).unwrap_err().code(),
        ManagedSkillInstallerErrorCode::Io
    );
    assert!(package_path(&root, request.package().revision()).is_dir());
    assert!(installed_catalog(&root).skills().is_empty());

    // This failpoint is reached only after the existing-package retry path
    // has verified the package and re-synced packages/v1.
    let resync_failure = ManagedSkillInstaller::with_failpoint(
        &root,
        ManagedSkillInstallerFailpoint::PackageParentSynced,
    );
    assert_eq!(
        resync_failure.install(&request).unwrap_err().code(),
        ManagedSkillInstallerErrorCode::Io
    );
    assert!(installed_catalog(&root).skills().is_empty());

    assert_eq!(
        ManagedSkillInstaller::new(&root)
            .unwrap()
            .install(&request)
            .unwrap(),
        ManagedSkillInstallOutcome::Installed
    );
}

#[test]
fn update_failpoints_expose_only_old_or_complete_new_packages() {
    let failpoints = [
        ManagedSkillInstallerFailpoint::PackageFileSynced,
        ManagedSkillInstallerFailpoint::PackagePublished,
        ManagedSkillInstallerFailpoint::PackageParentSynced,
        ManagedSkillInstallerFailpoint::ReceiptFileSynced,
        ManagedSkillInstallerFailpoint::ReceiptPublished,
        ManagedSkillInstallerFailpoint::ReceiptParentSynced,
    ];
    for failpoint in failpoints {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let install = install_request(INSTALLATION_ID, package("auditor", "OLD"));
        let old_revision = install.package().revision().clone();
        installer.install(&install).unwrap();
        let old_installation_revision = installation_revision(&root, install.installation_id());
        let update = ManagedSkillUpdateRequest::new(
            install.installation_id().clone(),
            old_installation_revision,
            package("auditor", "NEW"),
        );
        let target_revision = update.package().revision().clone();
        let failing = ManagedSkillInstaller::with_failpoint(&root, failpoint);

        let error = failing.update(&update).unwrap_err();
        let visible = installed_catalog(&root);
        assert_eq!(visible.skills().len(), 1);
        let visible_revision = visible.skills()[0].revision();
        if matches!(
            failpoint,
            ManagedSkillInstallerFailpoint::ReceiptPublished
                | ManagedSkillInstallerFailpoint::ReceiptParentSynced
        ) {
            assert_eq!(
                error.code(),
                ManagedSkillInstallerErrorCode::CommitIndeterminate
            );
            assert_eq!(visible_revision, &target_revision);
        } else {
            assert_eq!(error.code(), ManagedSkillInstallerErrorCode::Io);
            assert_eq!(visible_revision, &old_revision);
        }

        let retry = ManagedSkillInstaller::new(&root)
            .unwrap()
            .update(&update)
            .unwrap();
        assert!(matches!(
            retry.outcome(),
            ManagedSkillUpdateOutcome::Updated | ManagedSkillUpdateOutcome::AlreadyCurrent
        ));
        assert_eq!(
            installed_catalog(&root).skills()[0].revision(),
            &target_revision
        );
    }
}

#[test]
fn provenance_only_update_failpoints_expose_one_complete_receipt_generation() {
    for failpoint in [
        ManagedSkillInstallerFailpoint::PackageParentSynced,
        ManagedSkillInstallerFailpoint::ReceiptFileSynced,
        ManagedSkillInstallerFailpoint::ReceiptPublished,
        ManagedSkillInstallerFailpoint::ReceiptParentSynced,
    ] {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let package = package("auditor", "UNCHANGED_PACKAGE");
        let old_provenance = provenance("authority-a", Some("refresh-a"));
        let install = install_request_with_provenance(
            INSTALLATION_ID,
            package.clone(),
            old_provenance.clone(),
        );
        installer.install(&install).unwrap();
        let before = installed_receipt(&root, install.installation_id());
        let new_provenance = provenance("authority-b", Some("refresh-b"));
        let update = ManagedSkillUpdateRequest::with_provenance(
            install.installation_id().clone(),
            before.installation_revision.clone(),
            package,
            new_provenance.clone(),
        );
        let failing = ManagedSkillInstaller::with_failpoint(&root, failpoint);

        let error = failing.update(&update).unwrap_err();
        let visible = installed_receipt(&root, install.installation_id());
        assert_eq!(visible.package.revision, before.package.revision);
        if matches!(
            failpoint,
            ManagedSkillInstallerFailpoint::ReceiptPublished
                | ManagedSkillInstallerFailpoint::ReceiptParentSynced
        ) {
            assert_eq!(
                error.code(),
                ManagedSkillInstallerErrorCode::CommitIndeterminate
            );
            assert_eq!(visible.generation, 2);
            assert_eq!(visible.provenance, new_provenance);
            assert_ne!(visible.installation_revision, before.installation_revision);
        } else {
            assert_eq!(error.code(), ManagedSkillInstallerErrorCode::Io);
            assert_eq!(visible.generation, 1);
            assert_eq!(visible.provenance, old_provenance);
            assert_eq!(visible.installation_revision, before.installation_revision);
        }

        let retry = ManagedSkillInstaller::new(&root)
            .unwrap()
            .update(&update)
            .unwrap();
        assert!(matches!(
            retry.outcome(),
            ManagedSkillUpdateOutcome::Updated | ManagedSkillUpdateOutcome::AlreadyCurrent
        ));
        let converged = installed_receipt(&root, install.installation_id());
        assert_eq!(converged.generation, 2);
        assert_eq!(converged.provenance, new_provenance);
    }
}

#[test]
fn uninstall_failpoints_commit_absence_and_retry_converges() {
    for failpoint in [
        ManagedSkillInstallerFailpoint::UninstallRetirementPublished,
        ManagedSkillInstallerFailpoint::UninstallParentSynced,
    ] {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let install = install_request(INSTALLATION_ID, package("auditor", "DELETE"));
        installer.install(&install).unwrap();
        let uninstall = ManagedSkillUninstallRequest::new(
            install.installation_id().clone(),
            installation_revision(&root, install.installation_id()),
        );
        let failing = ManagedSkillInstaller::with_failpoint(&root, failpoint);

        let error = failing.uninstall(&uninstall).unwrap_err();
        assert_eq!(
            error.code(),
            ManagedSkillInstallerErrorCode::CommitIndeterminate
        );
        assert!(installed_catalog(&root).skills().is_empty());
        assert_eq!(
            fs::read_dir(root.join(RETIRED_INSTALLATIONS_DIRECTORY))
                .unwrap()
                .filter_map(Result::ok)
                .count(),
            1,
            "an indeterminate uninstall durably retires its installation identity"
        );
        assert_eq!(
            ManagedSkillInstaller::new(&root)
                .unwrap()
                .uninstall(&uninstall)
                .unwrap(),
            ManagedSkillUninstallOutcome::AlreadyAbsent
        );
        assert_eq!(
            fs::read_dir(root.join(RETIRED_INSTALLATIONS_DIRECTORY))
                .unwrap()
                .filter_map(Result::ok)
                .count(),
            1,
            "idempotent uninstall retries never delete the retired-ID ledger"
        );
    }
}

#[test]
fn every_error_after_retirement_publication_is_commit_indeterminate() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let install = install_request(INSTALLATION_ID, package("auditor", "DELETE"));
    installer.install(&install).unwrap();

    let transaction = installer.begin_transaction().unwrap();
    transaction
        .publish_retired_identity(install.installation_id(), true)
        .unwrap();
    let receipt_path = transaction.layout.receipt_path(install.installation_id());
    fs::remove_file(&receipt_path).unwrap();
    fs::create_dir(&receipt_path).unwrap();

    let error = transaction
        .retire_installation(install.installation_id(), true)
        .unwrap_err();
    assert_eq!(
        error.code(),
        ManagedSkillInstallerErrorCode::CommitIndeterminate
    );
    assert!(error.commit_may_have_succeeded());
    assert!(installed_catalog(&root).skills().is_empty());
}

#[test]
fn live_installation_capacity_rejects_growth_but_allows_full_capacity_update() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let original = install_request(INSTALLATION_ID, package("auditor", "CAPACITY_OLD"));
    installer.install(&original).unwrap();
    let original_installation_revision = installation_revision(&root, original.installation_id());
    let installations = root.join(INSTALLATIONS_DIRECTORY);
    let receipt_origin = original.package().origin();
    for value in 1..MAX_LIVE_INSTALLATIONS {
        let id = SkillInstallationId::parse(Uuid::from_u128(value as u128).to_string()).unwrap();
        let bytes = encode_receipt(
            &id,
            original.package().format_version(),
            original.package().revision(),
            receipt_origin,
            1_784_347_513_399,
        )
        .unwrap();
        fs::write(installations.join(format!("{id}.json")), bytes).unwrap();
    }
    assert_eq!(
        fs::read_dir(&installations).unwrap().count(),
        MAX_LIVE_INSTALLATIONS
    );

    let overflow = install_request(
        SECOND_INSTALLATION_ID,
        package("overflow", "MUST_NOT_INSTALL"),
    );
    let error = installer.install(&overflow).unwrap_err();
    assert_eq!(
        error.code(),
        ManagedSkillInstallerErrorCode::CapacityExceeded
    );
    assert!(matches!(
        error,
        ManagedSkillInstallerError::CapacityExceeded {
            capacity: ManagedSkillStoreCapacity::Installations,
            limit: MAX_LIVE_INSTALLATIONS,
        }
    ));
    assert_eq!(
        fs::read_dir(&installations).unwrap().count(),
        MAX_LIVE_INSTALLATIONS
    );

    let updated = package("auditor", "CAPACITY_NEW");
    let updated_revision = updated.revision().clone();
    let update = ManagedSkillUpdateRequest::new(
        original.installation_id().clone(),
        original_installation_revision,
        updated,
    );
    assert_eq!(
        installer.update(&update).unwrap(),
        ManagedSkillUpdateOutcome::Updated
    );
    assert_eq!(
        ManagedSkillStore::new(&root)
            .unwrap()
            .load_receipt(original.installation_id())
            .unwrap()
            .package
            .revision,
        updated_revision
    );
    assert_eq!(
        fs::read_dir(&installations).unwrap().count(),
        MAX_LIVE_INSTALLATIONS
    );
}

#[test]
fn absent_retirement_preserves_ledger_slots_for_every_live_installation() {
    assert!(
        ensure_retirement_marker_capacity(MAX_RETIRED_INSTALLATION_ENTRIES - 1, 1, true,).is_ok()
    );

    let error = ensure_retirement_marker_capacity(MAX_RETIRED_INSTALLATION_ENTRIES - 1, 1, false)
        .unwrap_err();
    assert_eq!(
        error.code(),
        ManagedSkillInstallerErrorCode::CapacityExceeded
    );
    assert!(matches!(
        error,
        ManagedSkillInstallerError::CapacityExceeded {
            capacity: ManagedSkillStoreCapacity::RetiredInstallationIds,
            limit: MAX_RETIRED_INSTALLATION_ENTRIES,
        }
    ));

    assert!(
        ensure_retirement_marker_capacity(MAX_RETIRED_INSTALLATION_ENTRIES - 1, 2, true,).is_err()
    );
}

#[test]
fn concurrent_mutations_are_serialized_by_the_installer() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installers = [
        Arc::new(ManagedSkillInstaller::new(&root).unwrap()),
        Arc::new(ManagedSkillInstaller::new(&root).unwrap()),
    ];
    let install = Arc::new(install_request(
        INSTALLATION_ID,
        package("auditor", "CONCURRENT"),
    ));
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for installer in installers {
        let install = Arc::clone(&install);
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            installer.install(&install).unwrap()
        }));
    }
    barrier.wait();
    let mut outcomes = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    outcomes.sort_by_key(|outcome| match outcome.outcome() {
        ManagedSkillInstallOutcome::Installed => 0,
        ManagedSkillInstallOutcome::AlreadyInstalled => 1,
    });
    assert_eq!(
        outcomes,
        vec![
            ManagedSkillInstallOutcome::Installed,
            ManagedSkillInstallOutcome::AlreadyInstalled
        ]
    );
    assert_eq!(installed_catalog(&root).skills().len(), 1);
}

#[test]
fn concurrent_updates_from_one_revision_commit_exactly_one_target() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let install = install_request(INSTALLATION_ID, package("auditor", "BASE"));
    installer.install(&install).unwrap();
    let base_revision = installation_revision(&root, install.installation_id());
    let first = Arc::new(ManagedSkillUpdateRequest::new(
        install.installation_id().clone(),
        base_revision.clone(),
        package("auditor", "TARGET_ONE"),
    ));
    let second = Arc::new(ManagedSkillUpdateRequest::new(
        install.installation_id().clone(),
        base_revision,
        package("auditor", "TARGET_TWO"),
    ));
    let target_revisions = [
        first.package().revision().clone(),
        second.package().revision().clone(),
    ];
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for (installer, request) in [
        (ManagedSkillInstaller::new(&root).unwrap(), first),
        (ManagedSkillInstaller::new(&root).unwrap(), second),
    ] {
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            installer.update(&request)
        }));
    }
    barrier.wait();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(
        results
            .iter()
            .filter(|result| {
                matches!(
                    result,
                    Ok(result)
                        if result.outcome() == &ManagedSkillUpdateOutcome::Updated
                )
            })
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(
                result,
                Err(error)
                    if error.code() == ManagedSkillInstallerErrorCode::RevisionConflict
            ))
            .count(),
        1
    );
    let visible = installed_catalog(&root);
    assert_eq!(visible.skills().len(), 1);
    assert!(target_revisions.contains(visible.skills()[0].revision()));
}

#[test]
fn stale_transaction_staging_is_cleaned_without_touching_foreign_entries() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let install = install_request(INSTALLATION_ID, package("auditor", "FIRST"));
    installer.install(&install).unwrap();
    let nonce = Uuid::new_v4();
    let receipt_stage = root.join(INSTALLATIONS_DIRECTORY).join(format!(
        ".receipt-{}-{nonce}.tmp",
        install.installation_id()
    ));
    fs::write(&receipt_stage, b"partial").unwrap();
    let digest = install
        .package()
        .revision()
        .as_str()
        .strip_prefix(PACKAGE_REVISION_PREFIX)
        .unwrap();
    let package_stage = root
        .join(PACKAGES_DIRECTORY)
        .join(PACKAGE_V1_DIRECTORY)
        .join(format!(".package-{digest}-{}.tmp", Uuid::new_v4()));
    fs::create_dir(&package_stage).unwrap();
    fs::create_dir(package_stage.join("references")).unwrap();
    fs::write(package_stage.join(SKILL_FILE_NAME), b"partial Skill").unwrap();
    fs::write(
        package_stage.join("references").join("guide.md"),
        b"partial",
    )
    .unwrap();
    let foreign_package_stage = package_stage
        .parent()
        .unwrap()
        .join(format!(".package-{digest}-not-a-uuid.tmp"));
    fs::create_dir(&foreign_package_stage).unwrap();
    fs::write(foreign_package_stage.join("keep"), b"foreign").unwrap();
    let unknown = root
        .join(INSTALLATIONS_DIRECTORY)
        .join(".third-party-state");
    fs::write(&unknown, b"preserve").unwrap();

    let second = install_request(SECOND_INSTALLATION_ID, package("second", "SECOND"));
    installer.install(&second).unwrap();

    assert!(!receipt_stage.exists());
    assert!(!package_stage.exists());
    assert_eq!(
        fs::read(foreign_package_stage.join("keep")).unwrap(),
        b"foreign"
    );
    assert!(unknown.exists());
}

#[test]
fn legacy_uninstall_tombstones_migrate_to_the_single_use_identity_ledger() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let first = install_request(INSTALLATION_ID, package("auditor", "FIRST"));
    installer.install(&first).unwrap();
    let receipt = root
        .join(INSTALLATIONS_DIRECTORY)
        .join(format!("{}.json", first.installation_id()));
    let legacy_tombstone = root.join(INSTALLATIONS_DIRECTORY).join(format!(
        ".uninstall-{}-{}.tombstone",
        first.installation_id(),
        Uuid::new_v4()
    ));
    fs::rename(&receipt, &legacy_tombstone).unwrap();

    let second = install_request(SECOND_INSTALLATION_ID, package("second", "SECOND"));
    installer.install(&second).unwrap();

    assert!(!legacy_tombstone.exists());
    assert!(root
        .join(RETIRED_INSTALLATIONS_DIRECTORY)
        .join(format!("{}.json", first.installation_id()))
        .is_file());
    assert_eq!(
        installer.install(&first).unwrap_err().code(),
        ManagedSkillInstallerErrorCode::InstallationRetired
    );
}

#[test]
fn tombstone_migration_retires_and_removes_a_simultaneously_live_identity() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let first = install_request(INSTALLATION_ID, package("auditor", "FIRST"));
    installer.install(&first).unwrap();
    let receipt = root
        .join(INSTALLATIONS_DIRECTORY)
        .join(format!("{}.json", first.installation_id()));
    let legacy_tombstone = root.join(INSTALLATIONS_DIRECTORY).join(format!(
        ".uninstall-{}-{}.tombstone",
        first.installation_id(),
        Uuid::new_v4()
    ));
    fs::copy(&receipt, &legacy_tombstone).unwrap();

    let second = install_request(SECOND_INSTALLATION_ID, package("second", "SECOND"));
    assert_eq!(
        installer.install(&second).unwrap(),
        ManagedSkillInstallOutcome::Installed
    );

    assert!(!receipt.exists());
    assert!(!legacy_tombstone.exists());
    assert!(root
        .join(RETIRED_INSTALLATIONS_DIRECTORY)
        .join(format!("{}.json", first.installation_id()))
        .is_file());
    assert_eq!(
        installer.install(&first).unwrap_err().code(),
        ManagedSkillInstallerErrorCode::InstallationRetired
    );
    assert_eq!(installed_catalog(&root).skills().len(), 1);
}

#[test]
fn durable_retirement_wins_if_a_live_receipt_reappears_after_crash() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let first = install_request(INSTALLATION_ID, package("auditor", "FIRST"));
    installer.install(&first).unwrap();
    let receipt = root
        .join(INSTALLATIONS_DIRECTORY)
        .join(format!("{}.json", first.installation_id()));
    let retired = root
        .join(RETIRED_INSTALLATIONS_DIRECTORY)
        .join(format!("{}.json", first.installation_id()));
    fs::copy(&receipt, &retired).unwrap();

    let index = ManagedSkillStore::new(&root)
        .unwrap()
        .scan_receipts()
        .unwrap();
    assert!(index.receipts.is_empty());
    assert_eq!(index.issues.len(), 1);
    assert!(receipt.is_file(), "read-only scans never repair the store");

    let second = install_request(SECOND_INSTALLATION_ID, package("second", "SECOND"));
    assert_eq!(
        installer.install(&second).unwrap(),
        ManagedSkillInstallOutcome::Installed
    );
    assert!(!receipt.exists());
    assert!(retired.is_file());
    assert_eq!(
        installer.install(&first).unwrap_err().code(),
        ManagedSkillInstallerErrorCode::InstallationRetired
    );
}

#[test]
fn staging_guards_only_delete_objects_owned_by_the_transaction() {
    let fixture = tempdir().unwrap();
    let foreign_file = fixture.path().join("reserved-but-not-created");
    fs::write(&foreign_file, b"foreign").unwrap();
    drop(StagingPath {
        path: foreign_file.clone(),
        parent: fixture.path().to_path_buf(),
        kind: StagingKind::File,
        armed: false,
    });
    assert_eq!(fs::read(&foreign_file).unwrap(), b"foreign");

    let owned_file = fixture.path().join(format!(
        ".receipt-{}-{}.tmp",
        installation_id(INSTALLATION_ID),
        Uuid::new_v4()
    ));
    fs::write(&owned_file, b"owned").unwrap();
    drop(StagingPath {
        path: owned_file.clone(),
        parent: fixture.path().to_path_buf(),
        kind: StagingKind::File,
        armed: true,
    });
    assert!(!owned_file.exists());

    let digest = "a".repeat(64);
    let owned_directory = fixture
        .path()
        .join(format!(".package-{digest}-{}.tmp", Uuid::new_v4()));
    fs::create_dir(&owned_directory).unwrap();
    fs::create_dir(owned_directory.join("references")).unwrap();
    fs::write(owned_directory.join(SKILL_FILE_NAME), b"owned").unwrap();
    fs::write(
        owned_directory.join("references").join("guide.md"),
        b"owned",
    )
    .unwrap();
    drop(StagingPath {
        path: owned_directory.clone(),
        parent: fixture.path().to_path_buf(),
        kind: StagingKind::Directory,
        armed: true,
    });
    assert!(!owned_directory.exists());

    let outside_parent = fixture.path().join("outside");
    fs::create_dir(&outside_parent).unwrap();
    let escaped = outside_parent.join(format!(".package-{digest}-{}.tmp", Uuid::new_v4()));
    fs::create_dir(&escaped).unwrap();
    fs::write(escaped.join(SKILL_FILE_NAME), b"foreign").unwrap();
    drop(StagingPath {
        path: escaped.clone(),
        parent: fixture.path().to_path_buf(),
        kind: StagingKind::Directory,
        armed: true,
    });
    assert_eq!(fs::read(escaped.join(SKILL_FILE_NAME)).unwrap(), b"foreign");
}

#[test]
fn startup_cleanup_rejects_owned_package_names_that_are_not_plain_directories() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let first = install_request(INSTALLATION_ID, package("auditor", "FIRST"));
    installer.install(&first).unwrap();
    let invalid_stage = root
        .join(PACKAGES_DIRECTORY)
        .join(PACKAGE_V1_DIRECTORY)
        .join(format!(
            ".package-{}-{}.tmp",
            "b".repeat(64),
            Uuid::new_v4()
        ));
    fs::write(&invalid_stage, b"do not delete").unwrap();

    let second = install_request(SECOND_INSTALLATION_ID, package("second", "SECOND"));
    let error = installer.install(&second).unwrap_err();

    assert_eq!(error.code(), ManagedSkillInstallerErrorCode::StoreCorrupt);
    assert_eq!(fs::read(&invalid_stage).unwrap(), b"do not delete");
    assert_eq!(installed_catalog(&root).skills().len(), 1);
}

#[test]
fn staging_cleanup_budget_is_inclusive_and_fails_closed() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let request = install_request(INSTALLATION_ID, package("auditor", "BUDGET"));
    installer.install(&request).unwrap();
    let installations = root.join(INSTALLATIONS_DIRECTORY);
    let package_version = root.join(PACKAGES_DIRECTORY).join(PACKAGE_V1_DIRECTORY);
    let package_v2 = root.join(PACKAGES_DIRECTORY).join(PACKAGE_V2_DIRECTORY);
    let package_v3 = root.join(PACKAGES_DIRECTORY).join(PACKAGE_V3_DIRECTORY);
    let layout = ManagedStoreLayout {
        root,
        installations: installations.clone(),
        retired_installations: fixture.path().join("unused-retired-installations"),
        package_v1: package_version,
        package_v2,
        package_v3,
    };
    let stale = installations.join(format!(
        ".receipt-{}-{}.tmp",
        request.installation_id(),
        Uuid::new_v4()
    ));
    fs::write(&stale, b"stale").unwrap();

    assert_eq!(
        cleanup_stale_transaction_entries_with_limit(&layout, 1)
            .unwrap_err()
            .code(),
        ManagedSkillInstallerErrorCode::InvalidStore
    );
    if !stale.exists() {
        fs::write(&stale, b"stale").unwrap();
    }
    cleanup_stale_transaction_entries_with_limit(&layout, 2).unwrap();
    assert!(!stale.exists());
}

#[test]
fn package_capacity_reserves_space_for_the_next_atomic_stage() {
    let fixture = tempdir().unwrap();
    let packages = fixture.path().join("packages");
    fs::create_dir(&packages).unwrap();

    ensure_directory_has_room(&packages, 2, ManagedSkillStoreCapacity::Packages).unwrap();
    fs::create_dir(packages.join("first")).unwrap();
    ensure_directory_has_room(&packages, 2, ManagedSkillStoreCapacity::Packages).unwrap();
    fs::create_dir(packages.join("second")).unwrap();
    let error =
        ensure_directory_has_room(&packages, 2, ManagedSkillStoreCapacity::Packages).unwrap_err();
    assert!(matches!(
        error,
        ManagedSkillInstallerError::CapacityExceeded {
            capacity: ManagedSkillStoreCapacity::Packages,
            limit: 2,
        }
    ));
}
