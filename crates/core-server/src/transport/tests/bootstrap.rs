use super::*;
use crate::adapters::skills_adapter;

#[test]
fn electron_app_data_root_is_canonical_and_has_priority_over_standalone_override() {
    let fixture = tempfile::tempdir().unwrap();
    let app_data_root = fixture.path().join("electron-profile");
    let legacy_database = fixture.path().join("legacy/storage.sqlite");
    let location = database_location_from_values(
        Some(app_data_root.clone()),
        Some(PathBuf::from("standalone/override.sqlite")),
        Some(legacy_database.clone()),
    )
    .unwrap();

    assert_eq!(
        location,
        DatabaseLocation {
            database_path: std::fs::canonicalize(app_data_root)
                .unwrap()
                .join("storage.sqlite"),
            legacy_database_path: Some(legacy_database),
        }
    );
}

#[test]
fn authoritative_app_data_root_rejects_empty_and_relative_paths() {
    let legacy = Some(PathBuf::from("/legacy/storage.sqlite"));
    assert_eq!(
        database_location_from_values(
            Some(PathBuf::new()),
            Some(PathBuf::from("override.sqlite")),
            legacy.clone(),
        )
        .unwrap_err()
        .kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert_eq!(
        database_location_from_values(
            Some(PathBuf::from("relative/profile")),
            Some(PathBuf::from("override.sqlite")),
            legacy,
        )
        .unwrap_err()
        .kind(),
        std::io::ErrorKind::InvalidInput
    );
}

#[cfg(unix)]
#[test]
fn authoritative_app_data_root_rejects_a_final_symlink() {
    use std::os::unix::fs::symlink;

    let fixture = tempfile::tempdir().unwrap();
    let real_root = fixture.path().join("real");
    let linked_root = fixture.path().join("linked");
    std::fs::create_dir(&real_root).unwrap();
    symlink(&real_root, &linked_root).unwrap();

    let error = database_location_from_values(Some(linked_root), None, None).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("symbolic link"));
}

#[test]
fn standalone_database_override_never_enables_implicit_profile_migration() {
    let location = database_location_from_values(
        None,
        Some(PathBuf::from("test-profile/override.sqlite")),
        Some(PathBuf::from("/legacy/profile/storage.sqlite")),
    )
    .unwrap();

    assert_eq!(
        location,
        DatabaseLocation {
            database_path: PathBuf::from("test-profile/override.sqlite"),
            legacy_database_path: None,
        }
    );
}

#[test]
fn installed_skill_store_is_a_sibling_of_the_effective_database() {
    assert_eq!(
        skill_store_root(std::path::Path::new("profile/storage.sqlite")),
        std::path::Path::new("profile/skills")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn development_credentials_are_private_siblings_of_the_effective_database() {
    assert_eq!(
        image_generation_development_credential_store_root(std::path::Path::new(
            "profile/storage.sqlite"
        )),
        std::path::Path::new("profile/image-generation-development-credentials-v1")
    );
}

#[test]
fn relative_database_override_is_absolutized_before_source_registration() {
    let current_directory = std::env::current_dir().unwrap();
    let database_path = absolute_path(PathBuf::from("profile/storage.sqlite")).unwrap();

    assert_eq!(
        database_path,
        current_directory.join("profile/storage.sqlite")
    );
    assert_eq!(
        skill_store_root(&database_path),
        current_directory.join("profile/skills")
    );
}

#[test]
fn database_instance_lock_enforces_one_core_server_and_releases_on_drop() {
    let fixture = tempfile::tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");

    let first = acquire_database_instance_lock(&database_path).unwrap();
    let error = acquire_database_instance_lock(&database_path).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(error.to_string().contains("another core-server"));

    drop(first);
    acquire_database_instance_lock(&database_path)
        .expect("dropping the owning core-server must release the database lock");
}

#[test]
fn legacy_storage_migration_is_a_noop_when_roots_are_the_same() {
    let fixture = tempfile::tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    create_real_database(&database_path);

    assert_eq!(
        migrate_legacy_storage_if_needed(&database_path, &database_path, false).unwrap(),
        LegacyStorageMigrationOutcome::SameRoot
    );
    assert!(database_path.exists());
}

#[test]
fn legacy_storage_is_copied_with_a_verified_sqlite_snapshot_and_no_sidecars() {
    let fixture = tempfile::tempdir().unwrap();
    let legacy_root = fixture.path().join("legacy");
    let target_root = fixture.path().join("electron");
    let legacy_database = legacy_root.join("storage.sqlite");
    let target_database = target_root.join("storage.sqlite");
    create_complete_legacy_profile(&legacy_root);
    create_empty_sqlite_sidecars(&legacy_database);
    let _target_lock = acquire_database_instance_lock(&target_database).unwrap();

    assert_eq!(
        migrate_legacy_storage_if_needed(&legacy_database, &target_database, true).unwrap(),
        LegacyStorageMigrationOutcome::Migrated
    );

    assert_eq!(
        std::fs::read(target_root.join("attachments/item.bin")).unwrap(),
        b"attachment"
    );
    assert_eq!(
        std::fs::read(target_root.join("skills/example/SKILL.md")).unwrap(),
        b"# Example"
    );
    assert_eq!(
        std::fs::read(target_root.join("image-generation-artifacts/image.png")).unwrap(),
        b"png"
    );
    assert_eq!(
        std::fs::read(target_root.join("image-generation-development-credentials-v1/credential"))
            .unwrap(),
        b"secret"
    );
    // Opening the migrated database exercises the real schema after the migration helper's own
    // quick_check. Do this after checking the deliberately orphaned attachment fixture because
    // normal startup maintenance removes attachment files without database records.
    drop(StorageService::open(&target_database).unwrap());
    for suffix in ["-journal", "-wal", "-shm"] {
        assert!(
            !sqlite_sidecar_path(&target_database, suffix).exists(),
            "SQLite sidecar {suffix} must not be copied"
        );
    }

    // The legacy profile remains available as a rollback copy.
    assert!(legacy_database.exists());
    assert_eq!(
        std::fs::read(legacy_root.join("attachments/item.bin")).unwrap(),
        b"attachment"
    );
    assert_eq!(
        migrate_legacy_storage_if_needed(&legacy_database, &target_database, true).unwrap(),
        LegacyStorageMigrationOutcome::TargetAlreadyInitialized
    );
}

#[test]
fn development_credentials_are_excluded_when_current_backend_does_not_use_them() {
    let fixture = tempfile::tempdir().unwrap();
    let legacy_root = fixture.path().join("legacy");
    let target_root = fixture.path().join("electron");
    create_complete_legacy_profile(&legacy_root);
    let target_database = target_root.join("storage.sqlite");
    let _target_lock = acquire_database_instance_lock(&target_database).unwrap();

    migrate_legacy_storage_if_needed(&legacy_root.join("storage.sqlite"), &target_database, false)
        .unwrap();
    assert!(!target_root
        .join("image-generation-development-credentials-v1")
        .exists());
}

#[test]
fn initialized_target_database_prevents_any_legacy_merge() {
    let fixture = tempfile::tempdir().unwrap();
    let legacy_root = fixture.path().join("legacy");
    let target_root = fixture.path().join("electron");
    create_complete_legacy_profile(&legacy_root);
    let target_database = target_root.join("storage.sqlite");
    create_real_database(&target_database);
    std::fs::create_dir_all(target_root.join("skills")).unwrap();
    std::fs::write(target_root.join("skills/new.txt"), b"new").unwrap();

    assert_eq!(
        migrate_legacy_storage_if_needed(
            &legacy_root.join("storage.sqlite"),
            &target_database,
            true,
        )
        .unwrap(),
        LegacyStorageMigrationOutcome::TargetAlreadyInitialized
    );
    assert_eq!(
        std::fs::read(target_root.join("skills/new.txt")).unwrap(),
        b"new"
    );
    assert!(!target_root.join("attachments/item.bin").exists());
}

#[test]
fn conflicting_precommit_target_entry_aborts_before_publishing_any_source_entry() {
    let fixture = tempfile::tempdir().unwrap();
    let legacy_root = fixture.path().join("legacy");
    let target_root = fixture.path().join("electron");
    create_complete_legacy_profile(&legacy_root);
    std::fs::create_dir_all(target_root.join("skills/example")).unwrap();
    std::fs::write(target_root.join("skills/example/SKILL.md"), b"# Different").unwrap();
    let target_database = target_root.join("storage.sqlite");
    let _target_lock = acquire_database_instance_lock(&target_database).unwrap();

    let error = migrate_legacy_storage_if_needed(
        &legacy_root.join("storage.sqlite"),
        &target_database,
        true,
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(error.to_string().contains("differ"));
    assert!(!target_root.join("attachments").exists());
    assert!(!target_database.exists());
    assert!(legacy_root.join("attachments/item.bin").exists());
}

#[test]
fn interrupted_precommit_copy_resumes_from_identical_published_directory() {
    let fixture = tempfile::tempdir().unwrap();
    let legacy_root = fixture.path().join("legacy");
    let target_root = fixture.path().join("electron");
    create_complete_legacy_profile(&legacy_root);
    copy_test_directory(
        &legacy_root.join("attachments"),
        &target_root.join("attachments"),
    );
    let target_database = target_root.join("storage.sqlite");
    let _target_lock = acquire_database_instance_lock(&target_database).unwrap();

    assert_eq!(
        migrate_legacy_storage_if_needed(
            &legacy_root.join("storage.sqlite"),
            &target_database,
            true,
        )
        .unwrap(),
        LegacyStorageMigrationOutcome::Migrated
    );
    assert_eq!(
        std::fs::read(target_root.join("attachments/item.bin")).unwrap(),
        b"attachment"
    );
    drop(StorageService::open(&target_database).unwrap());
    assert!(legacy_root.join("storage.sqlite").exists());
}

#[test]
fn target_sqlite_transient_is_rejected_before_directory_publication() {
    let fixture = tempfile::tempdir().unwrap();
    let legacy_root = fixture.path().join("legacy");
    let target_root = fixture.path().join("electron");
    create_complete_legacy_profile(&legacy_root);
    std::fs::create_dir_all(&target_root).unwrap();
    std::fs::write(target_root.join("storage.sqlite-shm"), b"orphan").unwrap();
    let target_database = target_root.join("storage.sqlite");
    let _target_lock = acquire_database_instance_lock(&target_database).unwrap();

    let error = migrate_legacy_storage_if_needed(
        &legacy_root.join("storage.sqlite"),
        &target_database,
        true,
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(error.to_string().contains("SQLite transient"));
    assert!(!target_root.join("attachments").exists());
    assert!(!target_database.exists());
}

#[cfg(unix)]
#[test]
fn legacy_storage_migration_rejects_symlinks_without_publishing_database() {
    use std::os::unix::fs::symlink;

    let fixture = tempfile::tempdir().unwrap();
    let legacy_root = fixture.path().join("legacy");
    let target_root = fixture.path().join("electron");
    create_real_database(&legacy_root.join("storage.sqlite"));
    let outside = fixture.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    symlink(&outside, legacy_root.join("skills")).unwrap();
    let target_database = target_root.join("storage.sqlite");
    let _target_lock = acquire_database_instance_lock(&target_database).unwrap();

    let error = migrate_legacy_storage_if_needed(
        &legacy_root.join("storage.sqlite"),
        &target_database,
        false,
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("symbolic link"));
    assert!(!target_database.exists());
}

#[test]
fn corrupt_legacy_sqlite_never_crosses_the_database_commit_boundary() {
    let fixture = tempfile::tempdir().unwrap();
    let legacy_root = fixture.path().join("legacy");
    let target_root = fixture.path().join("electron");
    std::fs::create_dir_all(&legacy_root).unwrap();
    std::fs::write(legacy_root.join("storage.sqlite"), b"not sqlite").unwrap();
    let target_database = target_root.join("storage.sqlite");
    let _target_lock = acquire_database_instance_lock(&target_database).unwrap();

    let error = migrate_legacy_storage_if_needed(
        &legacy_root.join("storage.sqlite"),
        &target_database,
        false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("SQLite"));
    assert!(!target_database.exists());
    assert!(std::fs::read_dir(&target_root).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains("core-storage-migration")));
}

#[test]
fn migration_cannot_snapshot_database_owned_by_another_core_server() {
    let fixture = tempfile::tempdir().unwrap();
    let legacy_root = fixture.path().join("legacy");
    let target_root = fixture.path().join("electron");
    let legacy_database = legacy_root.join("storage.sqlite");
    let target_database = target_root.join("storage.sqlite");
    create_real_database(&legacy_database);
    let _legacy_lock = acquire_database_instance_lock(&legacy_database).unwrap();
    let _target_lock = acquire_database_instance_lock(&target_database).unwrap();

    let error =
        migrate_legacy_storage_if_needed(&legacy_database, &target_database, false).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(error.to_string().contains("another core-server"));
    assert!(!target_database.exists());
}

#[test]
fn agent_skill_failures_preserve_structured_json_rpc_recovery_data() {
    let response = agent_service_error_response(
        JsonRpcId::Number(9),
        AgentServiceError::from(skills_adapter::missing_workspace_failure()),
    );

    assert_eq!(response["error"]["code"], -32000);
    assert_eq!(response["error"]["data"]["type"], "skillActivation");
    assert_eq!(response["error"]["data"]["code"], "invalidSelection");
    assert_eq!(response["error"]["data"]["recovery"], "rejectSelection");
}

fn create_real_database(database_path: &std::path::Path) {
    drop(StorageService::open(database_path).unwrap());
}

fn create_complete_legacy_profile(root: &std::path::Path) {
    create_real_database(&root.join("storage.sqlite"));
    std::fs::create_dir_all(root.join("attachments")).unwrap();
    std::fs::write(root.join("attachments/item.bin"), b"attachment").unwrap();
    std::fs::create_dir_all(root.join("skills/example")).unwrap();
    std::fs::write(root.join("skills/example/SKILL.md"), b"# Example").unwrap();
    std::fs::create_dir_all(root.join("image-generation-artifacts")).unwrap();
    std::fs::write(root.join("image-generation-artifacts/image.png"), b"png").unwrap();
    std::fs::create_dir_all(root.join("image-generation-development-credentials-v1")).unwrap();
    std::fs::write(
        root.join("image-generation-development-credentials-v1/credential"),
        b"secret",
    )
    .unwrap();
}

fn create_empty_sqlite_sidecars(database_path: &std::path::Path) {
    for suffix in ["-journal", "-wal", "-shm"] {
        std::fs::write(sqlite_sidecar_path(database_path, suffix), []).unwrap();
    }
}

fn sqlite_sidecar_path(database_path: &std::path::Path, suffix: &str) -> PathBuf {
    let mut path = database_path.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}

fn copy_test_directory(source: &std::path::Path, target: &std::path::Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_child = entry.path();
        let target_child = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_test_directory(&source_child, &target_child);
        } else {
            std::fs::copy(source_child, target_child).unwrap();
        }
    }
}
