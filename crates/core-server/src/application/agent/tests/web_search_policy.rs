use super::*;

pub(super) fn save_search_policy(
    storage: &StorageService,
    mode: &str,
    credential: CredentialMutation,
) {
    let mut settings = storage.load_model_settings_catalog().unwrap().unwrap();
    settings.search_mode = mode.to_string();
    let mut request =
        renderer_model_settings_update(storage, settings, ProviderProfileUpdate::Unchanged);
    request.tavily_api_key_mutation = credential;
    storage.save_model_settings_request(request).unwrap();
}

#[test]
fn live_web_search_policy_tracks_committed_settings_and_rejects_stale_cas() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let policy = service.web_search_policy_source();
    let initial = policy.snapshot().unwrap();
    assert!(!initial.enabled);
    assert!(policy
        .authorize_execution()
        .unwrap_err()
        .to_string()
        .contains("关闭"));

    let mut stale_settings = storage.load_model_settings_catalog().unwrap().unwrap();
    stale_settings.search_mode = "disabled".to_string();
    let stale_request =
        renderer_model_settings_update(&storage, stale_settings, ProviderProfileUpdate::Unchanged);
    save_search_policy(
        &storage,
        "auto",
        CredentialMutation::Replace {
            value: "HOST_SEARCH_CREDENTIAL_CANARY".to_string(),
        },
    );
    assert!(policy.snapshot().unwrap().available());
    assert!(!initial.available(), "a request snapshot remains immutable");
    assert!(storage.save_model_settings_request(stale_request).is_err());
    assert!(policy.snapshot().unwrap().available());
    assert_eq!(
        format!("{:?}", policy.authorize_execution().unwrap()),
        "WebSearchExecutionCredential([REDACTED])"
    );

    save_search_policy(&storage, "disabled", CredentialMutation::Keep);
    assert!(!policy.snapshot().unwrap().available());
    assert!(
        policy.authorize_execution().is_err(),
        "late execution must consult committed off policy"
    );
    save_search_policy(&storage, "tavily", CredentialMutation::Keep);
    assert!(policy.snapshot().unwrap().available());
    save_search_policy(&storage, "tavily", CredentialMutation::Clear);
    let unconfigured = policy.snapshot().unwrap();
    assert!(unconfigured.enabled);
    assert!(!unconfigured.credential_ready);
    assert!(policy.authorize_execution().is_err());
}
