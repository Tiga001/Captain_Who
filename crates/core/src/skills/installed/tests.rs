use super::*;
use crate::skills::digest::{
    package_revision, PACKAGE_REVISION_PREFIX, PACKAGE_REVISION_V2_PREFIX,
    PACKAGE_REVISION_V3_PREFIX,
};
use crate::skills::model::{SkillErrorCode, SkillRevision, SKILL_PACKAGE_FORMAT_VERSION};
use crate::skills::package::{
    PackageManifest, PackageManifestEntry, SkillPackagePath, PACKAGE_MANIFEST_FILE,
};
use crate::skills::workspace::{AGENTS_DIRECTORY, SKILLS_DIRECTORY, SKILL_FILE_NAME};
use crate::skills::{
    SkillResourceErrorCode, SkillResourceListOptions, SkillResourcePath,
    SkillResourceTextReadOptions, SkillsService,
};
use serde_json::json;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

const INSTALLATION_ID: &str = "01234567-89ab-4def-8123-456789abcdef";
const SECOND_INSTALLATION_ID: &str = "11111111-2222-4333-8444-555555555555";

fn skill_document(name: &str, marker: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: Managed fixture {name}.\n---\n# Instructions\n{marker}\n"
    )
}

fn write_package(root: &Path, source: &[u8]) -> SkillRevision {
    let revision = package_revision(source);
    let digest = revision
        .as_str()
        .strip_prefix(PACKAGE_REVISION_PREFIX)
        .unwrap();
    let package_root = root.join("packages").join("v1").join(digest);
    fs::create_dir_all(&package_root).unwrap();
    fs::write(package_root.join(SKILL_FILE_NAME), source).unwrap();
    revision
}

fn receipt_json(installation_id: &str, revision: &SkillRevision) -> serde_json::Value {
    receipt_json_with_format(installation_id, SKILL_PACKAGE_FORMAT_VERSION, revision)
}

fn receipt_json_with_format(
    installation_id: &str,
    format_version: u32,
    revision: &SkillRevision,
) -> serde_json::Value {
    json!({
        "schemaVersion": 1,
        "installationId": installation_id,
        "package": {
            "formatVersion": format_version,
            "revision": revision.as_str(),
            "entrypoint": "SKILL.md"
        },
        "origin": {
            "provider": "test-fixture",
            "reference": "fixture-without-secrets"
        },
        "installedAtUnixMs": 1_784_347_513_399_u64
    })
}

fn write_receipt_value(root: &Path, installation_id: &str, receipt: &serde_json::Value) {
    let installations = root.join("installations");
    fs::create_dir_all(&installations).unwrap();
    fs::write(
        installations.join(format!("{installation_id}.json")),
        receipt_bytes(receipt),
    )
    .unwrap();
}

fn receipt_bytes(receipt: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec_pretty(receipt).unwrap()
}

fn write_receipt(root: &Path, installation_id: &str, revision: &SkillRevision) {
    write_receipt_value(
        root,
        installation_id,
        &receipt_json(installation_id, revision),
    );
}

fn install(root: &Path, installation_id: &str, source: &[u8]) -> SkillRevision {
    let revision = write_package(root, source);
    write_receipt(root, installation_id, &revision);
    revision
}

fn package_root(root: &Path, revision: &SkillRevision) -> PathBuf {
    root.join("packages").join("v1").join(
        revision
            .as_str()
            .strip_prefix(PACKAGE_REVISION_PREFIX)
            .unwrap(),
    )
}

fn write_manifest_package(
    root: &Path,
    source: &[u8],
    resources: &[(&str, &[u8])],
) -> (u32, SkillRevision, PathBuf) {
    let mut entries = vec![PackageManifestEntry::from_bytes(
        SkillPackagePath::parse(SKILL_FILE_NAME).unwrap(),
        source,
    )];
    entries.extend(resources.iter().map(|(path, bytes)| {
        PackageManifestEntry::from_bytes(SkillPackagePath::parse(*path).unwrap(), bytes)
    }));
    let manifest = PackageManifest::new(entries).unwrap();
    let revision = manifest.revision();
    let (version, prefix) = match manifest.format_version() {
        2 => ("v2", PACKAGE_REVISION_V2_PREFIX),
        3 => ("v3", PACKAGE_REVISION_V3_PREFIX),
        other => panic!("unexpected manifest package format {other}"),
    };
    let digest = revision.as_str().strip_prefix(prefix).unwrap();
    let package_root = root.join("packages").join(version).join(digest);
    fs::create_dir_all(&package_root).unwrap();
    fs::write(
        package_root.join(PACKAGE_MANIFEST_FILE),
        manifest.encode().unwrap(),
    )
    .unwrap();
    fs::write(package_root.join(SKILL_FILE_NAME), source).unwrap();
    for (path, bytes) in resources {
        let destination = package_root.join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(destination, bytes).unwrap();
    }
    (manifest.format_version(), revision, package_root)
}

#[test]
fn installation_ids_require_canonical_lowercase_non_nil_uuids() {
    assert!(SkillInstallationId::parse(INSTALLATION_ID).is_ok());
    assert!(SkillInstallationId::parse(INSTALLATION_ID.to_uppercase()).is_err());
    assert!(SkillInstallationId::parse("00000000-0000-0000-0000-000000000000").is_err());
    assert!(SkillInstallationId::parse("0123456789ab4def8123456789abcdef").is_err());
    assert!(SkillInstallationId::parse("../receipt").is_err());
}

#[test]
fn installed_source_registration_rejects_relative_roots_without_touching_disk() {
    let relative = PathBuf::from("relative-managed-skill-store");
    let existed_before = relative.exists();

    let error = SkillsService::new()
        .with_installed_source(&relative)
        .unwrap_err();

    assert_eq!(error.code(), SkillErrorCode::InvalidSource);
    assert_eq!(relative.exists(), existed_before);
}

#[test]
fn missing_store_is_a_stable_empty_catalog() {
    let fixture = tempdir().unwrap();
    let missing = fixture.path().join("not-created");
    let service = SkillsService::new()
        .with_installed_source(&missing)
        .unwrap();

    let first = service.list().unwrap();
    let second = service.list().unwrap();

    assert!(first.skills().is_empty());
    assert!(first.diagnostics().is_empty());
    assert!(!first.truncated());
    assert_eq!(first.catalog_revision(), second.catalog_revision());
    assert!(
        !missing.exists(),
        "read-only discovery must not create the store"
    );
}

#[test]
fn lists_and_resolves_a_revision_bound_managed_package() {
    let fixture = tempdir().unwrap();
    let source = skill_document("installed-auditor", "INSTALLED_V1");
    let revision = install(fixture.path(), INSTALLATION_ID, source.as_bytes());
    let service = SkillsService::new()
        .with_installed_source(fixture.path())
        .unwrap();

    let catalog = service.list().unwrap();
    assert!(catalog.diagnostics().is_empty());
    assert_eq!(catalog.skills().len(), 1);
    let descriptor = &catalog.skills()[0];
    assert_eq!(
        descriptor.id().as_str(),
        format!("installed:user:{INSTALLATION_ID}")
    );
    assert_eq!(descriptor.revision(), &revision);
    assert_eq!(descriptor.source_kind(), SkillSourceKind::Installed);
    assert_eq!(descriptor.trust(), SkillTrust::Untrusted);
    assert_eq!(descriptor.activation_scope(), SkillActivationScope::Run);
    assert!(matches!(
        descriptor.provenance(),
        SkillProvenance::Installed {
            source_id,
            installation_id,
            relative_path,
        } if source_id.as_str() == USER_INSTALLED_SKILL_SOURCE_ID
            && installation_id.as_str() == INSTALLATION_ID
            && relative_path
                == &format!(
                    "packages/v1/{}/SKILL.md",
                    revision
                        .as_str()
                        .strip_prefix(PACKAGE_REVISION_PREFIX)
                        .unwrap()
                )
    ));

    let package = service.resolve(&descriptor.selection()).unwrap();
    assert_eq!(package.source_text(), source);
    assert!(package.instructions().contains("INSTALLED_V1"));
    assert_eq!(package.format_version(), SKILL_PACKAGE_FORMAT_VERSION);
    assert!(package.resources().is_empty());
}

#[test]
fn installation_identity_survives_update_and_old_snapshot_stays_frozen() {
    let fixture = tempdir().unwrap();
    let first_source = skill_document("installed-auditor", "VERSION_ONE");
    install(fixture.path(), INSTALLATION_ID, first_source.as_bytes());
    let service = SkillsService::new()
        .with_installed_source(fixture.path())
        .unwrap();
    let first_descriptor = service.list().unwrap().skills()[0].clone();
    let frozen = service.resolve(&first_descriptor.selection()).unwrap();

    let second_source = skill_document("installed-auditor", "VERSION_TWO");
    let second_revision = install(fixture.path(), INSTALLATION_ID, second_source.as_bytes());
    let second_descriptor = service.list().unwrap().skills()[0].clone();

    assert_eq!(first_descriptor.id(), second_descriptor.id());
    assert_ne!(first_descriptor.revision(), second_descriptor.revision());
    assert_eq!(second_descriptor.revision(), &second_revision);
    let stale = service.resolve(&first_descriptor.selection()).unwrap_err();
    assert_eq!(stale.code(), SkillErrorCode::Stale);
    assert_eq!(stale.actual_revision(), Some(&second_revision));
    assert!(frozen.instructions().contains("VERSION_ONE"));
    assert!(!frozen.source_text().contains("VERSION_TWO"));
}

#[test]
fn resource_sessions_and_restoration_stay_bound_to_old_packages_after_update_and_uninstall() {
    let fixture = tempdir().unwrap();
    let first_source = skill_document("resource-skill", "VERSION_ONE");
    let old_guide = b"old revision guide\n";
    let old_asset = b"old asset";
    let (first_format, first_revision, first_root) = write_manifest_package(
        fixture.path(),
        first_source.as_bytes(),
        &[
            ("assets/data.txt", old_asset.as_slice()),
            ("references/guide.md", old_guide.as_slice()),
        ],
    );
    write_receipt_value(
        fixture.path(),
        INSTALLATION_ID,
        &receipt_json_with_format(INSTALLATION_ID, first_format, &first_revision),
    );
    let service = SkillsService::new()
        .with_installed_source(fixture.path())
        .unwrap();
    let selection = service.list().unwrap().skills()[0].selection();
    let activated = service.activate(std::slice::from_ref(&selection)).unwrap();
    let session = service.resource_session(&activated).unwrap();
    let package = session.package_uris().remove(0);
    let listing = session
        .list(&package, &SkillResourceListOptions::default())
        .unwrap();
    assert_eq!(listing.entries().len(), 2);
    let guide_uri = package.resource(SkillResourcePath::parse("references/guide.md").unwrap());

    let second_source = skill_document("resource-skill", "VERSION_TWO");
    let (second_format, second_revision, _) = write_manifest_package(
        fixture.path(),
        second_source.as_bytes(),
        &[("references/guide.md", b"new revision guide\n")],
    );
    write_receipt_value(
        fixture.path(),
        INSTALLATION_ID,
        &receipt_json_with_format(INSTALLATION_ID, second_format, &second_revision),
    );

    let frozen = session
        .read_text(&guide_uri, SkillResourceTextReadOptions::default())
        .unwrap();
    assert_eq!(frozen.text(), "old revision guide\n");
    let restored_after_update = service
        .restore_resource_session(std::slice::from_ref(&selection))
        .unwrap();
    assert_eq!(
        restored_after_update
            .read_text(&guide_uri, SkillResourceTextReadOptions::default())
            .unwrap()
            .text(),
        "old revision guide\n"
    );

    fs::remove_file(
        fixture
            .path()
            .join("installations")
            .join(format!("{INSTALLATION_ID}.json")),
    )
    .unwrap();
    let restored_after_uninstall = service
        .restore_resource_session(std::slice::from_ref(&selection))
        .unwrap();
    assert_eq!(
        restored_after_uninstall
            .read_text(&guide_uri, SkillResourceTextReadOptions::default())
            .unwrap()
            .text(),
        "old revision guide\n"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let outside = tempdir().unwrap();
        let guide_path = first_root.join("references/guide.md");
        let outside_guide = outside.path().join("guide.md");
        fs::rename(&guide_path, &outside_guide).unwrap();
        symlink(&outside_guide, &guide_path).unwrap();
        let linked = restored_after_uninstall
            .read_text(&guide_uri, SkillResourceTextReadOptions::default())
            .unwrap_err();
        assert_eq!(linked.code(), SkillResourceErrorCode::IntegrityMismatch);
        assert!(!linked
            .message()
            .contains(&outside.path().display().to_string()));
        fs::remove_file(&guide_path).unwrap();
        fs::rename(&outside_guide, &guide_path).unwrap();
    }

    // A single-resource read does not eagerly load unrelated bytes.
    fs::write(
        first_root.join("assets/data.txt"),
        b"tampered unrelated asset",
    )
    .unwrap();
    assert_eq!(
        restored_after_uninstall
            .read_text(&guide_uri, SkillResourceTextReadOptions::default())
            .unwrap()
            .text(),
        "old revision guide\n"
    );

    fs::write(first_root.join("references/guide.md"), b"tampered guide").unwrap();
    let tampered = restored_after_uninstall
        .read_text(&guide_uri, SkillResourceTextReadOptions::default())
        .unwrap_err();
    assert_eq!(tampered.code(), SkillResourceErrorCode::IntegrityMismatch);
}

#[test]
fn stale_receipt_precedes_validation_of_the_new_package() {
    let fixture = tempdir().unwrap();
    let first = skill_document("installed-auditor", "FIRST");
    install(fixture.path(), INSTALLATION_ID, first.as_bytes());
    let service = SkillsService::new()
        .with_installed_source(fixture.path())
        .unwrap();
    let old_selection = service.list().unwrap().skills()[0].selection();

    let invalid = [0xff, 0xfe];
    let invalid_revision = write_package(fixture.path(), &invalid);
    write_receipt(fixture.path(), INSTALLATION_ID, &invalid_revision);

    let stale = service.resolve(&old_selection).unwrap_err();
    assert_eq!(stale.code(), SkillErrorCode::Stale);
    assert_eq!(stale.actual_revision(), Some(&invalid_revision));

    let current = SkillSelection::new(old_selection.skill_id().clone(), invalid_revision);
    let invalid_error = service.resolve(&current).unwrap_err();
    assert_eq!(invalid_error.code(), SkillErrorCode::InvalidSkill);
    assert_eq!(
        invalid_error.diagnostic_code(),
        Some(SkillDiagnosticCode::InvalidUtf8)
    );
}

#[test]
fn corrupt_receipt_and_package_are_isolated_from_healthy_installations() {
    let fixture = tempdir().unwrap();
    let valid = skill_document("healthy", "HEALTHY");
    install(fixture.path(), INSTALLATION_ID, valid.as_bytes());
    let broken_revision = write_package(
        fixture.path(),
        skill_document("broken", "BROKEN").as_bytes(),
    );
    write_receipt(fixture.path(), SECOND_INSTALLATION_ID, &broken_revision);
    fs::write(
        fixture
            .path()
            .join("installations")
            .join(format!("{SECOND_INSTALLATION_ID}.json")),
        b"{not-json",
    )
    .unwrap();
    fs::write(
        fixture.path().join("installations").join("not-a-uuid.json"),
        b"{}",
    )
    .unwrap();
    let service = SkillsService::new()
        .with_installed_source(fixture.path())
        .unwrap();

    let catalog = service.list().unwrap();

    assert_eq!(catalog.skills().len(), 1);
    assert_eq!(catalog.skills()[0].name(), "healthy");
    assert_eq!(
        catalog
            .diagnostics()
            .iter()
            .filter(|diagnostic| {
                diagnostic.code() == SkillDiagnosticCode::InvalidInstallationReceipt
            })
            .count(),
        2
    );
}

#[test]
fn hidden_staging_entries_are_ignored_without_changing_the_catalog() {
    let fixture = tempdir().unwrap();
    install(
        fixture.path(),
        INSTALLATION_ID,
        skill_document("healthy", "HEALTHY").as_bytes(),
    );
    fs::write(
        fixture.path().join("installations").join(".receipt.tmp"),
        b"partial writer state",
    )
    .unwrap();
    let service = SkillsService::new()
        .with_installed_source(fixture.path())
        .unwrap();

    let catalog = service.list().unwrap();

    assert_eq!(catalog.skills().len(), 1);
    assert!(catalog.diagnostics().is_empty());
    assert!(!catalog.truncated());
}

#[test]
fn installed_skill_reports_retired_model_tool_without_rewriting_or_hiding_it() {
    let fixture = tempdir().unwrap();
    let source = skill_document(
        "retired-writer",
        "Read the target, then call `write_file` with the replacement.",
    );
    install(fixture.path(), INSTALLATION_ID, source.as_bytes());
    let service = SkillsService::new()
        .with_installed_source(fixture.path())
        .unwrap();

    let catalog = service.list().unwrap();

    assert_eq!(catalog.skills().len(), 1);
    let diagnostic = catalog
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code() == SkillDiagnosticCode::UnsupportedToolReference)
        .expect("unsupported model tool reference should be diagnosed");
    assert_eq!(diagnostic.severity(), SkillDiagnosticSeverity::Warning);
    assert!(diagnostic.path().ends_with("/SKILL.md"));
    assert!(diagnostic.message().contains("`write_file`"));
    assert!(diagnostic.message().contains("`apply_patch`"));

    let resolved = service.resolve(&catalog.skills()[0].selection()).unwrap();
    assert!(resolved.instructions().contains("`write_file`"));
    assert_eq!(resolved.source_text(), source);
}

#[test]
fn installed_skill_reports_retired_model_tool_in_verified_resources() {
    let fixture = tempdir().unwrap();
    let source = skill_document("resource-lint", "Read the workflow reference.");
    let reference = b"Use `write_file` to publish the final output.\n";
    let (format_version, revision, package_root) = write_manifest_package(
        fixture.path(),
        source.as_bytes(),
        &[("references/workflow.md", reference)],
    );
    write_receipt_value(
        fixture.path(),
        INSTALLATION_ID,
        &receipt_json_with_format(INSTALLATION_ID, format_version, &revision),
    );
    let service = SkillsService::new()
        .with_installed_source(fixture.path())
        .unwrap();

    let catalog = service.list().unwrap();

    let diagnostic = catalog
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code() == SkillDiagnosticCode::UnsupportedToolReference)
        .expect("verified resource tool reference should be diagnosed");
    assert!(diagnostic.path().ends_with("references/workflow.md"));
    assert!(diagnostic.message().contains("`apply_patch`"));
    assert_eq!(
        fs::read(package_root.join("references/workflow.md")).unwrap(),
        reference
    );
}

#[test]
fn receipt_schema_and_filename_identity_fail_closed_per_installation() {
    let fixture = tempdir().unwrap();
    let revision = write_package(
        fixture.path(),
        skill_document("installed-auditor", "SAFE").as_bytes(),
    );
    let mut unknown_field = receipt_json(INSTALLATION_ID, &revision);
    unknown_field["unexpected"] = json!(true);
    write_receipt_value(fixture.path(), INSTALLATION_ID, &unknown_field);
    let service = SkillsService::new()
        .with_installed_source(fixture.path())
        .unwrap();

    let unknown_catalog = service.list().unwrap();
    assert!(unknown_catalog.skills().is_empty());
    assert_eq!(
        unknown_catalog.diagnostics()[0].code(),
        SkillDiagnosticCode::InvalidInstallationReceipt
    );

    fs::remove_file(
        fixture
            .path()
            .join("installations")
            .join(format!("{INSTALLATION_ID}.json")),
    )
    .unwrap();
    write_receipt_value(
        fixture.path(),
        SECOND_INSTALLATION_ID,
        &receipt_json(INSTALLATION_ID, &revision),
    );
    let mismatch_catalog = service.list().unwrap();
    assert!(mismatch_catalog.skills().is_empty());
    assert_eq!(
        mismatch_catalog.diagnostics()[0].code(),
        SkillDiagnosticCode::InvalidInstallationReceipt
    );
    assert!(mismatch_catalog.diagnostics()[0]
        .message()
        .contains("does not match"));
}

#[test]
fn malicious_revision_and_entrypoint_strings_never_become_store_paths() {
    let fixture = tempdir().unwrap();
    let source = skill_document("outside", "OUTSIDE_SECRET_CANARY");
    fs::write(fixture.path().join("outside-skill.md"), source).unwrap();
    let valid_revision = package_revision(b"unrelated valid bytes");
    let malicious_revisions = [
        "other-package-sha256-v1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_string(),
        format!("{PACKAGE_REVISION_PREFIX}{}", "A".repeat(64)),
        format!("{PACKAGE_REVISION_PREFIX}../{}", "a".repeat(61)),
        format!("{PACKAGE_REVISION_PREFIX}/tmp/{}", "a".repeat(59)),
        format!("{PACKAGE_REVISION_PREFIX}..\\{}", "a".repeat(61)),
    ];
    let service = SkillsService::new()
        .with_installed_source(fixture.path())
        .unwrap();

    for malicious_revision in malicious_revisions {
        let mut receipt = receipt_json(INSTALLATION_ID, &valid_revision);
        receipt["package"]["revision"] = json!(malicious_revision);
        write_receipt_value(fixture.path(), INSTALLATION_ID, &receipt);

        let catalog = service.list().unwrap();

        assert!(catalog.skills().is_empty());
        assert_eq!(catalog.diagnostics().len(), 1);
        assert_eq!(
            catalog.diagnostics()[0].code(),
            SkillDiagnosticCode::InvalidInstallationReceipt
        );
        assert!(!catalog.diagnostics()[0]
            .message()
            .contains("OUTSIDE_SECRET_CANARY"));
    }

    let mut entrypoint = receipt_json(INSTALLATION_ID, &valid_revision);
    entrypoint["package"]["entrypoint"] = json!("../../outside-skill.md");
    write_receipt_value(fixture.path(), INSTALLATION_ID, &entrypoint);
    let catalog = service.list().unwrap();
    assert!(catalog.skills().is_empty());
    assert_eq!(
        catalog.diagnostics()[0].code(),
        SkillDiagnosticCode::InvalidInstallationReceipt
    );
}

#[test]
fn package_catalog_byte_budget_is_shared_and_deterministic() {
    let fixture = tempdir().unwrap();
    let first_source = skill_document("first", "FIRST");
    let second_source = skill_document("second", "SECOND");
    install(fixture.path(), INSTALLATION_ID, first_source.as_bytes());
    install(
        fixture.path(),
        SECOND_INSTALLATION_ID,
        second_source.as_bytes(),
    );
    let mut source = InstalledSkillSource::new(fixture.path()).unwrap();
    source.set_package_catalog_byte_limit(first_source.len());

    let first = source.list().unwrap();
    let second = source.list().unwrap();

    assert_eq!(first, second);
    assert_eq!(first.skills().len(), 1);
    assert_eq!(first.skills()[0].name(), "first");
    assert!(first.truncated());
    assert_eq!(first.diagnostics().len(), 1);
    assert_eq!(
        first.diagnostics()[0].code(),
        SkillDiagnosticCode::CatalogTooLarge
    );
}

#[test]
fn receipt_entry_limit_is_inclusive_and_limit_plus_one_fails_closed() {
    let fixture = tempdir().unwrap();
    let source = skill_document("bounded", "BOUND");
    let revision = write_package(fixture.path(), source.as_bytes());
    write_receipt(fixture.path(), INSTALLATION_ID, &revision);
    let mut installed = InstalledSkillSource::new(fixture.path()).unwrap();
    installed.set_installation_entry_limit(1);

    let at_limit = installed.list().unwrap();
    assert_eq!(at_limit.skills().len(), 1);
    assert!(!at_limit.truncated());

    fs::write(
        fixture.path().join("installations").join(".receipt.tmp"),
        b"transient",
    )
    .unwrap();
    let with_transaction_headroom = installed.list().unwrap();
    assert_eq!(with_transaction_headroom.skills().len(), 1);
    assert!(!with_transaction_headroom.truncated());

    write_receipt(fixture.path(), SECOND_INSTALLATION_ID, &revision);
    let over_limit = installed.list().unwrap();
    assert!(over_limit.skills().is_empty());
    assert!(over_limit.truncated());
    assert_eq!(
        over_limit.diagnostics()[0].code(),
        SkillDiagnosticCode::TooManyEntries
    );
}

#[test]
fn installation_directory_budget_is_checked_even_when_the_target_sorts_first() {
    let fixture = tempdir().unwrap();
    install(
        fixture.path(),
        INSTALLATION_ID,
        skill_document("bounded", "BOUND").as_bytes(),
    );
    fs::write(
        fixture.path().join("installations").join("zz-foreign"),
        b"foreign",
    )
    .unwrap();
    let mut installed = InstalledSkillSource::new(fixture.path()).unwrap();
    installed.set_installation_directory_entry_limit(1);

    let catalog = installed.list().unwrap();

    assert!(catalog.skills().is_empty());
    assert!(catalog.truncated());
    assert_eq!(
        catalog.diagnostics()[0].code(),
        SkillDiagnosticCode::TooManyEntries
    );
}

#[test]
fn single_receipt_byte_limit_is_inclusive_and_limit_plus_one_is_rejected() {
    let fixture = tempdir().unwrap();
    let source = skill_document("bounded", "BOUND");
    let revision = write_package(fixture.path(), source.as_bytes());
    let receipt = receipt_json(INSTALLATION_ID, &revision);
    let receipt_len = receipt_bytes(&receipt).len();
    write_receipt_value(fixture.path(), INSTALLATION_ID, &receipt);
    let mut installed = InstalledSkillSource::new(fixture.path()).unwrap();

    installed.set_receipt_byte_limit(receipt_len);
    assert_eq!(installed.list().unwrap().skills().len(), 1);

    installed.set_receipt_byte_limit(receipt_len - 1);
    let over_limit = installed.list().unwrap();
    assert!(over_limit.skills().is_empty());
    assert_eq!(
        over_limit.diagnostics()[0].code(),
        SkillDiagnosticCode::InvalidInstallationReceipt
    );
}

#[test]
fn receipt_catalog_byte_limit_is_inclusive_and_shared_across_receipts() {
    let fixture = tempdir().unwrap();
    let source = skill_document("bounded", "BOUND");
    let revision = write_package(fixture.path(), source.as_bytes());
    let first = receipt_json(INSTALLATION_ID, &revision);
    let second = receipt_json(SECOND_INSTALLATION_ID, &revision);
    let exact_budget = receipt_bytes(&first).len() + receipt_bytes(&second).len();
    write_receipt_value(fixture.path(), INSTALLATION_ID, &first);
    write_receipt_value(fixture.path(), SECOND_INSTALLATION_ID, &second);
    let mut installed = InstalledSkillSource::new(fixture.path()).unwrap();

    installed.set_receipt_catalog_byte_limit(exact_budget);
    let at_limit = installed.list().unwrap();
    assert_eq!(at_limit.skills().len(), 2);
    assert!(!at_limit.truncated());

    installed.set_receipt_catalog_byte_limit(exact_budget - 1);
    let over_limit = installed.list().unwrap();
    assert_eq!(over_limit.skills().len(), 1);
    assert!(over_limit.truncated());
    assert_eq!(
        over_limit.diagnostics()[0].code(),
        SkillDiagnosticCode::CatalogTooLarge
    );
}

#[test]
fn differently_cased_managed_directories_are_never_accepted() {
    let fixture = tempdir().unwrap();
    let source = skill_document("wrong-case", "WRONG_CASE");
    let revision = package_revision(source.as_bytes());
    let digest = revision
        .as_str()
        .strip_prefix(PACKAGE_REVISION_PREFIX)
        .unwrap();
    let wrong_case_package = fixture.path().join("Packages").join("v1").join(digest);
    fs::create_dir_all(&wrong_case_package).unwrap();
    fs::write(wrong_case_package.join(SKILL_FILE_NAME), source).unwrap();
    write_receipt(fixture.path(), INSTALLATION_ID, &revision);
    let service = SkillsService::new()
        .with_installed_source(fixture.path())
        .unwrap();

    let catalog = service.list().unwrap();

    assert!(catalog.skills().is_empty());
    assert!(!catalog.diagnostics().is_empty());
}

#[test]
fn duplicate_installed_names_preserve_both_ids_and_use_relative_diagnostics() {
    let fixture = tempdir().unwrap();
    let source = skill_document("shared-name", "SHARED");
    let revision = write_package(fixture.path(), source.as_bytes());
    write_receipt(fixture.path(), INSTALLATION_ID, &revision);
    write_receipt(fixture.path(), SECOND_INSTALLATION_ID, &revision);
    let service = SkillsService::new()
        .with_installed_source(fixture.path())
        .unwrap();

    let catalog = service.list().unwrap();

    assert_eq!(catalog.skills().len(), 2);
    assert_ne!(catalog.skills()[0].id(), catalog.skills()[1].id());
    let diagnostic = catalog
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code() == SkillDiagnosticCode::DuplicateName)
        .unwrap();
    assert_eq!(diagnostic.severity(), SkillDiagnosticSeverity::Warning);
    assert_eq!(diagnostic.path(), "installations");
    assert!(diagnostic.message().contains(INSTALLATION_ID));
    assert!(diagnostic.message().contains(SECOND_INSTALLATION_ID));
    assert!(!diagnostic
        .message()
        .contains(&fixture.path().display().to_string()));
}

#[test]
fn detects_digest_mismatch_missing_packages_and_forbidden_siblings() {
    let fixture = tempdir().unwrap();
    let source = skill_document("installed-auditor", "ORIGINAL");
    let revision = install(fixture.path(), INSTALLATION_ID, source.as_bytes());
    let service = SkillsService::new()
        .with_installed_source(fixture.path())
        .unwrap();

    fs::write(
        package_root(fixture.path(), &revision).join(SKILL_FILE_NAME),
        skill_document("installed-auditor", "TAMPERED"),
    )
    .unwrap();
    let mismatch = service.list().unwrap();
    assert!(mismatch.skills().is_empty());
    assert_eq!(
        mismatch.diagnostics()[0].code(),
        SkillDiagnosticCode::PackageRevisionMismatch
    );

    fs::remove_dir_all(package_root(fixture.path(), &revision)).unwrap();
    let missing = service.list().unwrap();
    assert!(missing.skills().is_empty());
    assert_eq!(
        missing.diagnostics()[0].code(),
        SkillDiagnosticCode::MissingSkillFile
    );

    fs::create_dir_all(package_root(fixture.path(), &revision)).unwrap();
    fs::write(
        package_root(fixture.path(), &revision).join(SKILL_FILE_NAME),
        source,
    )
    .unwrap();
    fs::write(
        package_root(fixture.path(), &revision).join("reference.md"),
        "not supported in package v1",
    )
    .unwrap();
    let sibling = service.list().unwrap();
    assert!(sibling.skills().is_empty());
    assert_eq!(
        sibling.diagnostics()[0].code(),
        SkillDiagnosticCode::UnexpectedPackageEntry
    );
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_receipts_and_package_files_without_reading_targets() {
    use std::os::unix::fs::symlink;

    let fixture = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let source = skill_document("installed-auditor", "SECRET_CANARY");
    let revision = install(fixture.path(), INSTALLATION_ID, source.as_bytes());
    let receipt = fixture
        .path()
        .join("installations")
        .join(format!("{INSTALLATION_ID}.json"));
    let outside_receipt = outside.path().join("receipt.json");
    fs::rename(&receipt, &outside_receipt).unwrap();
    symlink(&outside_receipt, &receipt).unwrap();
    let service = SkillsService::new()
        .with_installed_source(fixture.path())
        .unwrap();
    let receipt_catalog = service.list().unwrap();
    assert!(receipt_catalog.skills().is_empty());
    assert_eq!(
        receipt_catalog.diagnostics()[0].code(),
        SkillDiagnosticCode::SymlinkNotAllowed
    );
    assert!(!receipt_catalog.diagnostics()[0]
        .message()
        .contains("SECRET_CANARY"));

    fs::remove_file(&receipt).unwrap();
    fs::rename(outside_receipt, &receipt).unwrap();
    let skill_file = package_root(fixture.path(), &revision).join(SKILL_FILE_NAME);
    let outside_skill = outside.path().join(SKILL_FILE_NAME);
    fs::rename(&skill_file, &outside_skill).unwrap();
    symlink(&outside_skill, &skill_file).unwrap();
    let package_catalog = service.list().unwrap();
    assert!(package_catalog.skills().is_empty());
    assert_eq!(
        package_catalog.diagnostics()[0].code(),
        SkillDiagnosticCode::SymlinkNotAllowed
    );
    assert!(!package_catalog.diagnostics()[0]
        .message()
        .contains("SECRET_CANARY"));
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_store_root_and_package_directory() {
    use std::os::unix::fs::symlink;

    let fixture = tempdir().unwrap();
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("secret"), "ROOT_SECRET_CANARY").unwrap();
    let linked_root = fixture.path().join("linked-store");
    symlink(outside.path(), &linked_root).unwrap();
    let linked_service = SkillsService::new()
        .with_installed_source(&linked_root)
        .unwrap();

    let linked_catalog = linked_service.list().unwrap();
    assert!(linked_catalog.skills().is_empty());
    assert_eq!(
        linked_catalog.diagnostics()[0].code(),
        SkillDiagnosticCode::SourceUnavailable
    );
    assert!(!linked_catalog.diagnostics()[0]
        .message()
        .contains("ROOT_SECRET_CANARY"));

    let store = fixture.path().join("store");
    let source = skill_document("installed-auditor", "PACKAGE_SECRET_CANARY");
    let revision = install(&store, INSTALLATION_ID, source.as_bytes());
    let digest_root = package_root(&store, &revision);
    let moved = outside.path().join("moved-package");
    fs::rename(&digest_root, &moved).unwrap();
    symlink(&moved, &digest_root).unwrap();
    let service = SkillsService::new().with_installed_source(&store).unwrap();

    let catalog = service.list().unwrap();
    assert!(catalog.skills().is_empty());
    assert_eq!(
        catalog.diagnostics()[0].code(),
        SkillDiagnosticCode::SymlinkNotAllowed
    );
    assert!(!catalog.diagnostics()[0]
        .message()
        .contains("PACKAGE_SECRET_CANARY"));
}

#[test]
fn three_real_sources_aggregate_and_activate_in_selection_order() {
    let fixture = tempdir().unwrap();
    let store = fixture.path().join("managed");
    let workspace = fixture.path().join("workspace");
    install(
        &store,
        INSTALLATION_ID,
        skill_document("installed-auditor", "INSTALLED").as_bytes(),
    );
    let workspace_skill = workspace
        .join(AGENTS_DIRECTORY)
        .join(SKILLS_DIRECTORY)
        .join("workspace-auditor");
    fs::create_dir_all(&workspace_skill).unwrap();
    fs::write(
        workspace_skill.join(SKILL_FILE_NAME),
        skill_document("workspace-auditor", "WORKSPACE"),
    )
    .unwrap();
    let service = SkillsService::new()
        .with_bundled_source()
        .unwrap()
        .with_installed_source(&store)
        .unwrap();
    let catalog = service
        .list_with_workspace("workspace", &workspace)
        .unwrap();

    assert_eq!(catalog.skills().len(), 9);
    let selection = |kind| {
        catalog
            .skills()
            .iter()
            .find(|skill| skill.source_kind() == kind)
            .unwrap()
            .selection()
    };
    let ordered = [
        selection(SkillSourceKind::Installed),
        selection(SkillSourceKind::Workspace),
        selection(SkillSourceKind::Bundled),
    ];
    let activated = service
        .activate_workspace("workspace", &workspace, &ordered)
        .unwrap();
    assert_eq!(
        activated
            .skills()
            .iter()
            .map(ResolvedSkillPackage::source_kind)
            .collect::<Vec<_>>(),
        vec![
            SkillSourceKind::Installed,
            SkillSourceKind::Workspace,
            SkillSourceKind::Bundled,
        ]
    );
}
