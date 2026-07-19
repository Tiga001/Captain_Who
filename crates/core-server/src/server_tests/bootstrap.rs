use super::*;

#[test]
fn installed_skill_store_is_a_sibling_of_the_effective_database() {
    assert_eq!(
        skill_store_root(std::path::Path::new("profile/storage.sqlite")),
        std::path::Path::new("profile/skills")
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
