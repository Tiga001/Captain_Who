use super::*;
use crate::image_generation::{
    CredentialStore, CredentialStoreBackend, CredentialStoreError, InMemoryCredentialStore,
};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct PausingCredentials {
    delegate: InMemoryCredentialStore,
    before_replace: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl CredentialStore for PausingCredentials {
    fn backend(&self) -> CredentialStoreBackend {
        CredentialStoreBackend::InMemoryV1
    }
    fn replace(
        &self,
        reference: &CredentialReference,
        secret: CredentialSecret,
    ) -> Result<(), CredentialStoreError> {
        assert_ne!(std::thread::current().name(), Some("core-sqlite-writer"));
        let callback = self.before_replace.lock().unwrap().take();
        if let Some(callback) = callback {
            callback();
        }
        self.delegate.replace(reference, secret)
    }
    fn get(
        &self,
        reference: &CredentialReference,
    ) -> Result<Option<CredentialSecret>, CredentialStoreError> {
        assert_ne!(std::thread::current().name(), Some("core-sqlite-writer"));
        self.delegate.get(reference)
    }
    fn delete(
        &self,
        reference: &CredentialReference,
    ) -> Result<CredentialDeleteOutcome, CredentialStoreError> {
        assert_ne!(std::thread::current().name(), Some("core-sqlite-writer"));
        self.delegate.delete(reference)
    }
}

fn fixture() -> (
    tempfile::TempDir,
    Arc<StorageService>,
    Arc<PausingCredentials>,
) {
    let directory = tempfile::tempdir().unwrap();
    let credentials = Arc::new(PausingCredentials::default());
    let service = StorageService::open_with_model_credentials(
        &directory.path().join("settings.sqlite"),
        credentials.clone(),
    )
    .unwrap();
    (directory, Arc::new(service), credentials)
}

fn model_settings() -> ModelSettingsRecord {
    ModelSettingsRecord {
        api_url: "https://fixture.invalid/v1".into(),
        api_token: "fixture-token".into(),
        search_mode: "disabled".into(),
        tavily_api_key: String::new(),
        models: vec![ModelConfigRecord {
            id: "fixture-model".into(),
            provider_model_id: "fixture-model".into(),
            display_name: "Fixture Model".into(),
            api_url_override: None,
            api_token_override: None,
            supports_image: false,
            context_window_tokens: Some(128_000),
            provider_profile_config: crate::ProviderProfileConfig::generic_for_dialect(
                crate::ProviderProtocolDialect::OpenAiChatCompletions,
            ),
            input_price: "0".into(),
            cached_input_price: String::new(),
            output_price: "0".into(),
            enabled: true,
        }],
    }
}

#[test]
fn stalled_native_credential_write_does_not_occupy_sqlite_writer() {
    let (_directory, service, credentials) = fixture();
    let (entered, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    *credentials.before_replace.lock().unwrap() = Some(Box::new(move || {
        entered.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
    }));
    let save_service = service.clone();
    let save = std::thread::spawn(move || save_service.save_model_settings(model_settings()));
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let other_service = service.clone();
    let (done, received) = mpsc::channel();
    let other = std::thread::spawn(move || {
        let mut preferences = other_service.load_ui_preferences().unwrap();
        preferences.profile_display_name = "Unrelated write".into();
        done.send(other_service.save_ui_preferences(preferences))
            .unwrap();
    });
    let unrelated = received.recv_timeout(Duration::from_secs(2));
    release.send(()).unwrap();
    save.join().unwrap().unwrap();
    other.join().unwrap();
    assert_eq!(
        unrelated
            .expect("credentials must not retain writer capacity")
            .unwrap()
            .profile_display_name,
        "Unrelated write"
    );
    let snapshot = service.load_model_settings_snapshot().unwrap().unwrap();
    assert_eq!(snapshot.settings.api_token, "fixture-token");
    let reader = service.state.read_connection().unwrap();
    for table in [
        "model_provider_credential_staging",
        "model_provider_credential_cleanup",
    ] {
        assert_eq!(
            reader
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn model_snapshot_and_projection_readers_do_not_wait_for_sqlite_writer() {
    let (_directory, service, _credentials) = fixture();
    service.save_model_settings(model_settings()).unwrap();
    let (entered, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let writer_service = service.clone();
    let writer = std::thread::spawn(move || {
        writer_service
            .state
            .write_sync(1024, StorageWriteClass::Normal, move |connection| {
                let transaction = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .unwrap();
                entered.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
                transaction.commit().unwrap();
            })
            .unwrap();
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let reader_service = service.clone();
    let (done, received) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let snapshot = reader_service
            .load_model_settings_snapshot_for_model("fixture-model", false)
            .unwrap()
            .unwrap();
        let projection = reader_service
            .load_model_projection_cached()
            .unwrap()
            .unwrap();
        done.send((snapshot, projection)).unwrap();
    });
    let result = received.recv_timeout(Duration::from_secs(2));
    release.send(()).unwrap();
    writer.join().unwrap();
    reader.join().unwrap();
    let (snapshot, projection) = result.expect("model reads must use the WAL pool");
    assert_eq!(
        snapshot.configuration_revision,
        projection.configuration_revision
    );
    assert_eq!(
        snapshot.settings.models[0].id,
        projection.models[0].model.id
    );
    assert!(projection.models[0].execution.is_available());
}

#[test]
fn failed_preferences_write_leaves_previous_committed_permissions_visible() {
    let (_directory, service, _credentials) = fixture();
    let mut preferences = service.load_ui_preferences().unwrap();
    preferences.full_permission_enabled = true;
    service.save_ui_preferences(preferences.clone()).unwrap();
    service.state.write_sync(1024, StorageWriteClass::Normal, |connection| {
        connection.execute_batch("CREATE TRIGGER reject_preferences BEFORE UPDATE ON ui_preferences BEGIN SELECT RAISE(ABORT, 'injected preference write failure'); END;")
    }).unwrap().unwrap();
    preferences.full_permission_enabled = false;
    assert!(service
        .save_ui_preferences(preferences)
        .unwrap_err()
        .contains("injected preference write failure"));
    assert!(
        service
            .load_ui_preferences()
            .unwrap()
            .full_permission_enabled
    );
}
