use super::*;
use crate::adapters::skills_adapter;

#[test]
fn electron_app_data_root_is_canonical_and_has_priority_over_standalone_paths() {
    let fixture = tempfile::tempdir().unwrap();
    let app_data_root = fixture.path().join("electron-profile");
    let database_path = database_path_from_values(
        Some(app_data_root.clone()),
        Some(PathBuf::from("standalone/override.sqlite")),
        Some(PathBuf::from("standalone/default.sqlite")),
    )
    .unwrap();

    assert_eq!(
        database_path,
        std::fs::canonicalize(app_data_root)
            .unwrap()
            .join("storage.sqlite")
    );
}

#[test]
fn authoritative_app_data_root_rejects_empty_and_relative_paths() {
    let standalone_default = Some(PathBuf::from("/standalone/default/storage.sqlite"));
    assert_eq!(
        database_path_from_values(
            Some(PathBuf::new()),
            Some(PathBuf::from("override.sqlite")),
            standalone_default.clone(),
        )
        .unwrap_err()
        .kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert_eq!(
        database_path_from_values(
            Some(PathBuf::from("relative/profile")),
            Some(PathBuf::from("override.sqlite")),
            standalone_default,
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

    let error = database_path_from_values(Some(linked_root), None, None).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("symbolic link"));
}

#[test]
fn standalone_database_override_has_priority_over_the_platform_default() {
    let database_path = database_path_from_values(
        None,
        Some(PathBuf::from("test-profile/override.sqlite")),
        Some(PathBuf::from("/standalone/default/storage.sqlite")),
    )
    .unwrap();

    assert_eq!(database_path, PathBuf::from("test-profile/override.sqlite"));
}

#[test]
fn standalone_uses_the_platform_default_without_an_override() {
    let default = PathBuf::from("/standalone/default/storage.sqlite");
    assert_eq!(
        database_path_from_values(None, None, Some(default.clone())).unwrap(),
        default
    );
}

#[test]
fn standalone_fails_clearly_when_the_platform_default_is_unavailable() {
    let error = database_path_from_values(None, None, None).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert!(error.to_string().contains("platform user-data environment"));
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
