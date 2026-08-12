use super::*;

#[test]
fn install_update_and_uninstall_have_stable_idempotent_semantics() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let original = package("auditor", "ORIGINAL");
    let original_revision = original.revision().clone();
    let request = install_request(INSTALLATION_ID, original.clone());

    assert_eq!(
        installer.install(&request).unwrap(),
        ManagedSkillInstallOutcome::Installed
    );
    assert_eq!(
        installer.install(&request).unwrap(),
        ManagedSkillInstallOutcome::AlreadyInstalled
    );
    let receipt = ManagedSkillStore::new(&root)
        .unwrap()
        .load_receipt(request.installation_id())
        .unwrap();
    assert_eq!(receipt.installed_at_unix_ms, 1_784_347_513_399);
    assert_eq!(receipt.package.revision, original_revision);
    let original_installation_revision = receipt.installation_revision.clone();

    let second = install_request(SECOND_INSTALLATION_ID, original);
    assert_eq!(
        installer.install(&second).unwrap(),
        ManagedSkillInstallOutcome::Installed
    );
    assert_eq!(installed_catalog(&root).skills().len(), 2);

    let updated = package("auditor", "UPDATED");
    let updated_revision = updated.revision().clone();
    let update = ManagedSkillUpdateRequest::new(
        request.installation_id().clone(),
        original_installation_revision.clone(),
        updated,
    );
    assert_eq!(
        installer.update(&update).unwrap(),
        ManagedSkillUpdateOutcome::Updated
    );
    assert_eq!(
        installer.update(&update).unwrap(),
        ManagedSkillUpdateOutcome::AlreadyCurrent
    );
    let receipt = ManagedSkillStore::new(&root)
        .unwrap()
        .load_receipt(request.installation_id())
        .unwrap();
    assert_eq!(receipt.package.revision, updated_revision);
    let updated_installation_revision = receipt.installation_revision.clone();
    assert_eq!(
        receipt.installed_at_unix_ms, 1_784_347_513_399,
        "updates preserve the original installation timestamp"
    );

    let conflict = ManagedSkillUpdateRequest::new(
        request.installation_id().clone(),
        original_installation_revision.clone(),
        package("auditor", "CONFLICT"),
    );
    assert_eq!(
        installer.update(&conflict).unwrap_err().code(),
        ManagedSkillInstallerErrorCode::RevisionConflict
    );
    let wrong_uninstall = ManagedSkillUninstallRequest::new(
        request.installation_id().clone(),
        original_installation_revision,
    );
    assert_eq!(
        installer.uninstall(&wrong_uninstall).unwrap_err().code(),
        ManagedSkillInstallerErrorCode::RevisionConflict
    );

    let uninstall = ManagedSkillUninstallRequest::new(
        request.installation_id().clone(),
        updated_installation_revision,
    );
    assert_eq!(
        installer.uninstall(&uninstall).unwrap(),
        ManagedSkillUninstallOutcome::Uninstalled
    );
    assert_eq!(
        installer.uninstall(&uninstall).unwrap(),
        ManagedSkillUninstallOutcome::AlreadyAbsent
    );
    let retired = root
        .join(RETIRED_INSTALLATIONS_DIRECTORY)
        .join(format!("{}.json", request.installation_id()));
    assert!(retired.is_file());
    assert_eq!(
        ManagedSkillInstaller::new(&root)
            .unwrap()
            .install(&request)
            .unwrap_err()
            .code(),
        ManagedSkillInstallerErrorCode::InstallationRetired,
        "an old install retry must not resurrect an explicitly uninstalled Skill"
    );
    assert!(package_path(&root, &updated_revision).is_dir());
    let catalog = installed_catalog(&root);
    assert_eq!(catalog.skills().len(), 1);
    assert_eq!(
        catalog.skills()[0].source_kind(),
        SkillSourceKind::Installed
    );
    assert!(catalog.skills()[0]
        .id()
        .as_str()
        .starts_with(USER_INSTALLED_SKILL_SOURCE_ID));
}

#[test]
fn provenance_only_update_advances_generation_and_identical_retry_is_inert() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let package = package("auditor", "SAME_BYTES");
    let install = install_request_with_provenance(
        INSTALLATION_ID,
        package.clone(),
        provenance("commit-a", Some("main")),
    );
    let installed = installer.install(&install).unwrap();
    assert_eq!(installed, ManagedSkillInstallOutcome::Installed);
    let before = installed_receipt(&root, install.installation_id());
    let conflicting_install = install_request_with_provenance(
        INSTALLATION_ID,
        package.clone(),
        provenance("commit-b", Some("main")),
    );
    assert_eq!(
        installer.install(&conflicting_install).unwrap_err().code(),
        ManagedSkillInstallerErrorCode::InstallationExists
    );

    let update = ManagedSkillUpdateRequest::with_provenance(
        install.installation_id().clone(),
        before.installation_revision.clone(),
        package,
        provenance("commit-b", Some("main")),
    );
    let updated = installer.update(&update).unwrap();
    assert_eq!(updated, ManagedSkillUpdateOutcome::Updated);
    let after = installed_receipt(&root, install.installation_id());
    assert_eq!(after.package.revision, before.package.revision);
    assert_eq!(after.generation, 2);
    assert_ne!(after.installation_revision, before.installation_revision);
    assert_eq!(after.installed_at_unix_ms, before.installed_at_unix_ms);
    assert!(after.updated_at_unix_ms >= before.installed_at_unix_ms);
    assert_eq!(after.provenance, *update.provenance());

    let retry = installer.update(&update).unwrap();
    assert_eq!(retry, ManagedSkillUpdateOutcome::AlreadyCurrent);
    let retried = installed_receipt(&root, install.installation_id());
    assert_eq!(retried.generation, after.generation);
    assert_eq!(retried.installation_revision, after.installation_revision);
    assert_eq!(retried.updated_at_unix_ms, after.updated_at_unix_ms);
}

#[test]
fn installation_generation_prevents_aba_from_authorizing_a_different_target() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let package = package("auditor", "SAME_BYTES");
    let install = install_request_with_provenance(
        INSTALLATION_ID,
        package.clone(),
        provenance("authority-a", None),
    );
    installer.install(&install).unwrap();
    let generation_one = installed_receipt(&root, install.installation_id());

    let to_b = ManagedSkillUpdateRequest::with_provenance(
        install.installation_id().clone(),
        generation_one.installation_revision.clone(),
        package.clone(),
        provenance("authority-b", None),
    );
    installer.update(&to_b).unwrap();
    let generation_two = installed_receipt(&root, install.installation_id());
    let back_to_a = ManagedSkillUpdateRequest::with_provenance(
        install.installation_id().clone(),
        generation_two.installation_revision,
        package.clone(),
        provenance("authority-a", None),
    );
    installer.update(&back_to_a).unwrap();
    let generation_three = installed_receipt(&root, install.installation_id());
    assert_eq!(generation_three.generation, 3);
    assert_ne!(
        generation_three.installation_revision,
        generation_one.installation_revision
    );

    let stale_to_c = ManagedSkillUpdateRequest::with_provenance(
        install.installation_id().clone(),
        generation_one.installation_revision,
        package,
        provenance("authority-c", None),
    );
    let error = installer.update(&stale_to_c).unwrap_err();
    assert_eq!(
        error.code(),
        ManagedSkillInstallerErrorCode::RevisionConflict
    );
    assert_eq!(
        installed_receipt(&root, install.installation_id()).installation_revision,
        generation_three.installation_revision
    );
}

#[test]
fn updating_a_v1_receipt_naturally_migrates_it_to_v2() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let package = package("auditor", "LEGACY_BYTES");
    let install = install_request(INSTALLATION_ID, package.clone());
    installer.install(&install).unwrap();
    let legacy = serde_json::json!({
        "schemaVersion": 1,
        "installationId": INSTALLATION_ID,
        "package": {
            "formatVersion": package.format_version(),
            "revision": package.revision().as_str(),
            "entrypoint": "SKILL.md"
        },
        "origin": {
            "provider": package.origin().provider(),
            "reference": package.origin().reference()
        },
        "installedAtUnixMs": 1_784_347_513_399_u64
    });
    fs::write(
        root.join(INSTALLATIONS_DIRECTORY)
            .join(format!("{INSTALLATION_ID}.json")),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    let before = installed_receipt(&root, install.installation_id());
    assert!(before.is_legacy_v1());
    assert_eq!(before.generation, 1);

    let update = ManagedSkillUpdateRequest::new(
        install.installation_id().clone(),
        before.installation_revision.clone(),
        package,
    );
    assert_eq!(
        installer.update(&update).unwrap(),
        ManagedSkillUpdateOutcome::Updated
    );
    let after = installed_receipt(&root, install.installation_id());
    assert!(!after.is_legacy_v1());
    assert_eq!(after.receipt_schema_version, 2);
    assert_eq!(after.generation, 2);
    assert_ne!(after.installation_revision, before.installation_revision);
    assert_eq!(after.installed_at_unix_ms, before.installed_at_unix_ms);
}

#[test]
fn exhausted_generation_fails_closed_without_replacing_the_receipt() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let package = package("auditor", "MAX_GENERATION");
    let install = install_request_with_provenance(
        INSTALLATION_ID,
        package.clone(),
        provenance("authority-a", None),
    );
    installer.install(&install).unwrap();
    let max_generation = InstalledSkillReceipt::new_v2(
        install.installation_id().clone(),
        u64::MAX,
        package.format_version(),
        package.revision().clone(),
        install.provenance().clone(),
        1_784_347_513_399,
        1_784_347_513_399,
    )
    .unwrap();
    fs::write(
        root.join(INSTALLATIONS_DIRECTORY)
            .join(format!("{INSTALLATION_ID}.json")),
        encode_receipt_v2(&max_generation).unwrap(),
    )
    .unwrap();

    let update = ManagedSkillUpdateRequest::with_provenance(
        install.installation_id().clone(),
        max_generation.installation_revision.clone(),
        package,
        provenance("authority-b", None),
    );
    assert_eq!(
        installer.update(&update).unwrap_err().code(),
        ManagedSkillInstallerErrorCode::StoreCorrupt
    );
    let still_current = installed_receipt(&root, install.installation_id());
    assert_eq!(still_current.generation, u64::MAX);
    assert_eq!(
        still_current.installation_revision,
        max_generation.installation_revision
    );
}

#[test]
fn v2_install_update_and_v1_downgrade_share_the_existing_transaction() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let original = package("resourceful", "V1_INSTRUCTIONS");
    let install = install_request(INSTALLATION_ID, original);
    installer.install(&install).unwrap();
    let original_installation_revision = installation_revision(&root, install.installation_id());

    let resourceful = package_v2("resourceful", "V2_INSTRUCTIONS", b"GUIDE_V2");
    assert_eq!(
        resourceful.format_version(),
        SKILL_PACKAGE_FORMAT_VERSION_V2
    );
    assert!(resourceful
        .revision()
        .as_str()
        .starts_with(PACKAGE_REVISION_V2_PREFIX));
    let v2_revision = resourceful.revision().clone();
    let update = ManagedSkillUpdateRequest::new(
        install.installation_id().clone(),
        original_installation_revision,
        resourceful.clone(),
    );
    assert_eq!(
        installer.update(&update).unwrap(),
        ManagedSkillUpdateOutcome::Updated
    );
    assert_eq!(
        installer.update(&update).unwrap(),
        ManagedSkillUpdateOutcome::AlreadyCurrent
    );
    let v2_installation_revision = installation_revision(&root, install.installation_id());

    let digest = v2_revision
        .as_str()
        .strip_prefix(PACKAGE_REVISION_V2_PREFIX)
        .unwrap();
    let v2_root = root
        .join(PACKAGES_DIRECTORY)
        .join(PACKAGE_V2_DIRECTORY)
        .join(digest);
    assert_eq!(
        fs::read(v2_root.join("references/guide.md")).unwrap(),
        b"GUIDE_V2"
    );
    assert!(v2_root.join(PACKAGE_MANIFEST_FILE).is_file());

    let service = SkillsService::new().with_installed_source(&root).unwrap();
    let catalog = service.list().unwrap();
    let resolved = service.resolve(&catalog.skills()[0].selection()).unwrap();
    assert_eq!(resolved.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V2);
    assert_eq!(resolved.resources().len(), 2);
    assert_eq!(
        resolved
            .resources()
            .get("references/guide.md")
            .unwrap()
            .byte_length(),
        8
    );

    let downgraded = package("resourceful", "V1_AGAIN");
    let downgrade_revision = downgraded.revision().clone();
    let downgrade = ManagedSkillUpdateRequest::new(
        install.installation_id().clone(),
        v2_installation_revision,
        downgraded,
    );
    assert_eq!(
        installer.update(&downgrade).unwrap(),
        ManagedSkillUpdateOutcome::Updated
    );
    let catalog = service.list().unwrap();
    assert_eq!(catalog.skills()[0].revision(), &downgrade_revision);
    let resolved = service.resolve(&catalog.skills()[0].selection()).unwrap();
    assert_eq!(resolved.format_version(), SKILL_PACKAGE_FORMAT_VERSION);
    assert!(resolved.resources().is_empty());
}

#[test]
fn v3_install_roundtrips_generic_resources_through_the_managed_store() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let package = package_v3("portable", "USE_ALL_FILES");
    assert_eq!(package.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V3);
    assert!(package
        .revision()
        .as_str()
        .starts_with(PACKAGE_REVISION_V3_PREFIX));
    let request = install_request(INSTALLATION_ID, package.clone());

    assert_eq!(
        installer.install(&request).unwrap(),
        ManagedSkillInstallOutcome::Installed
    );
    assert_eq!(
        installer.install(&request).unwrap(),
        ManagedSkillInstallOutcome::AlreadyInstalled
    );

    let digest = package
        .revision()
        .as_str()
        .strip_prefix(PACKAGE_REVISION_V3_PREFIX)
        .unwrap();
    let package_root = root
        .join(PACKAGES_DIRECTORY)
        .join(PACKAGE_V3_DIRECTORY)
        .join(digest);
    assert_eq!(
        fs::read(package_root.join("README.md")).unwrap(),
        b"ROOT_RESOURCE"
    );
    assert_eq!(
        fs::read(package_root.join("agents/openai.yaml")).unwrap(),
        b"interface: chat"
    );
    assert!(package_root.join(PACKAGE_MANIFEST_FILE).is_file());

    let service = SkillsService::new().with_installed_source(&root).unwrap();
    let catalog = service.list().unwrap();
    let resolved = service.resolve(&catalog.skills()[0].selection()).unwrap();
    assert_eq!(resolved.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V3);
    assert_eq!(resolved.resources(), &package.resource_index());
    assert_eq!(
        resolved.resources().get("README.md").unwrap().kind(),
        crate::skills::SkillResourceKind::Other
    );

    let package_ref = InstalledPackageRef::from_format_and_revision(
        package.format_version(),
        package.revision().clone(),
    )
    .unwrap();
    let snapshot = ManagedSkillStore::new(root)
        .unwrap()
        .load_complete_package(&package_ref)
        .unwrap();
    assert_eq!(
        snapshot.resource_bytes,
        vec![
            ("README.md".to_string(), b"ROOT_RESOURCE".to_vec()),
            (
                "agents/openai.yaml".to_string(),
                b"interface: chat".to_vec()
            ),
            ("references/guide.md".to_string(), b"GUIDE".to_vec()),
        ]
    );
}

#[test]
fn updates_can_cross_v2_and_v3_format_boundaries_without_changing_identity() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let v2 = package_v2("portable", "V2", b"V2_GUIDE");
    let install = install_request(INSTALLATION_ID, v2);
    installer.install(&install).unwrap();
    let v2_installation_revision = installation_revision(&root, install.installation_id());

    let v3 = package_v3("portable", "V3");
    let v3_revision = v3.revision().clone();
    let upgrade = ManagedSkillUpdateRequest::new(
        install.installation_id().clone(),
        v2_installation_revision,
        v3,
    );
    assert_eq!(
        installer.update(&upgrade).unwrap(),
        ManagedSkillUpdateOutcome::Updated
    );
    let service = SkillsService::new().with_installed_source(&root).unwrap();
    let catalog = service.list().unwrap();
    let skill_id = catalog.skills()[0].id().clone();
    assert_eq!(catalog.skills()[0].revision(), &v3_revision);
    assert_eq!(
        service
            .resolve(&catalog.skills()[0].selection())
            .unwrap()
            .format_version(),
        SKILL_PACKAGE_FORMAT_VERSION_V3
    );
    let v3_installation_revision = installation_revision(&root, install.installation_id());

    let v2_again = package_v2("portable", "V2_AGAIN", b"UPDATED_GUIDE");
    let v2_again_revision = v2_again.revision().clone();
    let downgrade = ManagedSkillUpdateRequest::new(
        install.installation_id().clone(),
        v3_installation_revision,
        v2_again,
    );
    assert_eq!(
        installer.update(&downgrade).unwrap(),
        ManagedSkillUpdateOutcome::Updated
    );
    let catalog = service.list().unwrap();
    assert_eq!(catalog.skills()[0].id(), &skill_id);
    assert_eq!(catalog.skills()[0].revision(), &v2_again_revision);
    assert_eq!(
        service
            .resolve(&catalog.skills()[0].selection())
            .unwrap()
            .format_version(),
        SKILL_PACKAGE_FORMAT_VERSION_V2
    );
}

#[test]
fn v2_retry_deeply_verifies_an_existing_content_addressed_tree() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let package = package_v2("resourceful", "READ_RESOURCE", b"ORIGINAL");
    let request = install_request(INSTALLATION_ID, package.clone());
    installer.install(&request).unwrap();

    let digest = package
        .revision()
        .as_str()
        .strip_prefix(PACKAGE_REVISION_V2_PREFIX)
        .unwrap();
    fs::write(
        root.join(PACKAGES_DIRECTORY)
            .join(PACKAGE_V2_DIRECTORY)
            .join(digest)
            .join("references/guide.md"),
        b"TAMPERED",
    )
    .unwrap();

    let error = installer.install(&request).unwrap_err();
    assert_eq!(error.code(), ManagedSkillInstallerErrorCode::StoreCorrupt);
}

#[test]
fn v2_resolve_deeply_verifies_every_revision_bound_resource() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let package = package_v2("resourceful", "READ_RESOURCE", b"ORIGINAL");
    let request = install_request(INSTALLATION_ID, package.clone());
    installer.install(&request).unwrap();

    let service = SkillsService::new().with_installed_source(&root).unwrap();
    let selection = service.list().unwrap().skills()[0].selection();
    let digest = package
        .revision()
        .as_str()
        .strip_prefix(PACKAGE_REVISION_V2_PREFIX)
        .unwrap();
    fs::remove_file(
        root.join(PACKAGES_DIRECTORY)
            .join(PACKAGE_V2_DIRECTORY)
            .join(digest)
            .join("references/guide.md"),
    )
    .unwrap();

    // Catalog discovery is intentionally manifest-only, but activation
    // must not expose a resource index for an incomplete package.
    assert_eq!(service.list().unwrap().skills().len(), 1);
    let error = service.resolve(&selection).unwrap_err();
    assert_eq!(error.code(), crate::skills::SkillErrorCode::InvalidSkill);
    assert_eq!(
        error.diagnostic_code(),
        Some(crate::skills::SkillDiagnosticCode::MissingSkillFile)
    );
}

#[test]
fn mixed_v1_v2_receipts_list_and_resolve_together() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let v1 = install_request(INSTALLATION_ID, package("plain", "PLAIN"));
    let v2 = install_request(
        SECOND_INSTALLATION_ID,
        package_v2("resourceful", "RESOURCEFUL", b"GUIDE"),
    );
    installer.install(&v1).unwrap();
    installer.install(&v2).unwrap();

    let v1_receipt = ManagedSkillStore::new(&root)
        .unwrap()
        .load_receipt(v1.installation_id())
        .unwrap();
    let v2_receipt = ManagedSkillStore::new(&root)
        .unwrap()
        .load_receipt(v2.installation_id())
        .unwrap();
    assert_eq!(
        v1_receipt.package.format_version,
        SKILL_PACKAGE_FORMAT_VERSION
    );
    assert_eq!(
        v2_receipt.package.format_version,
        SKILL_PACKAGE_FORMAT_VERSION_V2
    );

    let service = SkillsService::new().with_installed_source(&root).unwrap();
    let catalog = service.list().unwrap();
    assert_eq!(catalog.skills().len(), 2);
    let resolved = catalog
        .skills()
        .iter()
        .map(|descriptor| service.resolve(&descriptor.selection()).unwrap())
        .collect::<Vec<_>>();
    assert!(resolved
        .iter()
        .any(|package| package.format_version() == SKILL_PACKAGE_FORMAT_VERSION));
    assert!(resolved.iter().any(|package| {
        package.format_version() == SKILL_PACKAGE_FORMAT_VERSION_V2
            && package.resources().len() == 2
    }));
}

#[test]
fn corrupt_v2_manifest_is_isolated_by_the_read_only_source() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let installer = ManagedSkillInstaller::new(&root).unwrap();
    let package = package_v2("resourceful", "READ_RESOURCE", b"GUIDE");
    let request = install_request(INSTALLATION_ID, package.clone());
    installer.install(&request).unwrap();
    let digest = package
        .revision()
        .as_str()
        .strip_prefix(PACKAGE_REVISION_V2_PREFIX)
        .unwrap();
    let manifest_path = root
        .join(PACKAGES_DIRECTORY)
        .join(PACKAGE_V2_DIRECTORY)
        .join(digest)
        .join(PACKAGE_MANIFEST_FILE);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["unexpected"] = serde_json::json!(true);
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

    let catalog = installed_catalog(&root);
    assert!(catalog.skills().is_empty());
    assert_eq!(
        catalog.diagnostics()[0].code(),
        crate::skills::SkillDiagnosticCode::InvalidPackageManifest
    );
}

#[test]
fn v2_failpoints_preserve_the_same_receipt_linearization_boundary() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("store");
    let request = install_request(
        INSTALLATION_ID,
        package_v2("resourceful", "READ_RESOURCE", b"GUIDE"),
    );
    let precommit = ManagedSkillInstaller::with_failpoint(
        &root,
        ManagedSkillInstallerFailpoint::PackagePublished,
    );
    assert_eq!(
        precommit.install(&request).unwrap_err().code(),
        ManagedSkillInstallerErrorCode::Io
    );
    assert!(installed_catalog(&root).skills().is_empty());

    let indeterminate = ManagedSkillInstaller::with_failpoint(
        &root,
        ManagedSkillInstallerFailpoint::ReceiptPublished,
    );
    assert_eq!(
        indeterminate.install(&request).unwrap_err().code(),
        ManagedSkillInstallerErrorCode::CommitIndeterminate
    );
    let catalog = installed_catalog(&root);
    assert_eq!(catalog.skills().len(), 1);
    assert_eq!(
        SkillsService::new()
            .with_installed_source(&root)
            .unwrap()
            .resolve(&catalog.skills()[0].selection())
            .unwrap()
            .resources()
            .len(),
        2
    );
}
