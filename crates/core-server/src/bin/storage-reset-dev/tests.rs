use super::*;
use mycopilot_core::image_generation::{
    CredentialReference, CredentialSecret, CredentialStore, DevelopmentFileCredentialStore,
    ImageGenerationConfigurationService,
};
use mycopilot_core::storage::image_generation_repository::IMAGE_GENERATION_PROFILE_SCHEMA_VERSION;
use mycopilot_core::storage::models::{
    BrowserDownloadLocationMode, BrowserDownloadSettingsUpdate, BrowserLinkOpenTarget,
    ChatConversationRecord, ChatMessageRecord, ModelConfigRecord, ModelSettingsRecord,
    ProjectRecord, BROWSER_DOWNLOAD_SCHEMA_VERSION,
};
use mycopilot_core::storage::notification_repository::{
    NotificationSettingsUpdate, NotificationSettingsUpdateOutcome,
};
use mycopilot_core::ProviderProtocolDialect;
use mycopilot_mcp_client::{
    McpApprovalMode, McpServerConfig, McpServerId, McpServerScope, McpStdioConfig,
    McpTransportConfig,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

fn options(root: &Path, confirm_reset: bool) -> ResetOptions {
    ResetOptions {
        app_data_root: root.to_path_buf(),
        confirm_reset,
        configuration_source: None,
    }
}

fn open_test_storage(database: &Path) -> StorageService {
    StorageService::open_with_model_credentials(
        database,
        development_model_credential_store(database).unwrap(),
    )
    .unwrap()
}

#[test]
fn historical_catalog_derivation_keeps_exact_pre_v50_fingerprints() {
    for (schema, fingerprint) in [
        (
            canonical_schema_before_context_profiles(),
            COLLABORATION_CONFIGURATION_SOURCE_FINGERPRINT,
        ),
        (
            canonical_schema_before_multi_folder_projects(),
            CONTEXT_PROFILE_CONFIGURATION_SOURCE_FINGERPRINT,
        ),
        (
            canonical_schema_v47(),
            LOCAL_TOKEN_CONFIGURATION_SOURCE_FINGERPRINT,
        ),
    ] {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(&schema).unwrap();
        assert_eq!(
            storage_catalog_fingerprint(&connection).unwrap(),
            fingerprint
        );
        assert_eq!(connection.query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'agent_collaboration_event_activities'",
                [], |row| row.get::<_, i64>(0),
            ).unwrap(), 0);
    }
}

#[test]
fn removing_v60_fixture_objects_restores_exact_v59_catalog_and_seeded_data() {
    let fixture = tempfile::tempdir().unwrap();
    populated_storage(fixture.path(), "v60-fixture-only-secret");
    let connection = Connection::open(fixture.path().join(DATABASE_FILE_NAME)).unwrap();
    let configuration = snapshot_exact_configuration_tables(&connection).unwrap();
    remove_v60_fixture_objects(&connection);
    let previous = Connection::open_in_memory().unwrap();
    previous
        .execute_batch(
            include_str!("../../../../core/src/storage/canonical_schema.sql")
                .split_once("-- Indexed chronological history projection, schema v60.")
                .expect("v60 history schema suffix")
                .0,
        )
        .unwrap();
    assert_eq!(
        storage_catalog_fingerprint(&connection).unwrap(),
        storage_catalog_fingerprint(&previous).unwrap()
    );
    assert_eq!(
        snapshot_exact_configuration_tables(&connection).unwrap(),
        configuration
    );
    assert_eq!(
        count_rows_if_table_exists(&connection, "messages").unwrap(),
        2
    );
    assert!(pragma_rows(&connection, "PRAGMA foreign_key_check")
        .unwrap()
        .is_empty());
}

#[test]
fn removing_v53_fixture_objects_retains_exact_v52_catalog_and_seeded_data() {
    let fixture = tempfile::tempdir().unwrap();
    populated_storage(fixture.path(), "v53-fixture-only-secret");
    let connection = Connection::open(fixture.path().join(DATABASE_FILE_NAME)).unwrap();
    let configuration = snapshot_exact_configuration_tables(&connection).unwrap();
    remove_v53_fixture_objects(&connection);
    assert_eq!(
        storage_catalog_fingerprint(&connection).unwrap(),
        "sha256:615c17481af79b297a930607a7ca0d8e45a3825527ac19f74bf4a64b747b4cc7"
    );
    assert_eq!(
        snapshot_exact_configuration_tables(&connection).unwrap(),
        configuration
    );
    assert_eq!(
        count_rows_if_table_exists(&connection, "messages").unwrap(),
        2
    );
    assert!(pragma_rows(&connection, "PRAGMA foreign_key_check")
        .unwrap()
        .is_empty());
}

fn downgrade_fixture_to_exact_v35(database: &Path) {
    let connection = Connection::open(database).unwrap();
    drop_async_fixture_schema(&connection);
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             DROP TRIGGER human_interaction_sync_stop_fence;
             DROP TABLE human_interaction_suspensions;
             DROP TABLE human_interaction_deliveries;
             DROP TABLE human_interaction_responses;
             DROP TABLE human_interaction_requests;
             DROP TABLE human_interaction_settings;
             PRAGMA user_version = 35;",
        )
        .unwrap();
    assert_eq!(
        storage_catalog_fingerprint(&connection).unwrap(),
        PREVIOUS_CONFIGURATION_SOURCE_FINGERPRINT
    );
}

fn model_settings_with_secret(secret: &str) -> ModelSettingsRecord {
    ModelSettingsRecord {
        api_url: "https://api.example.test/v1/chat/completions".to_string(),
        api_token: secret.to_string(),
        search_mode: "tavily".to_string(),
        tavily_api_key: format!("search-{secret}"),
        models: vec![ModelConfigRecord {
            id: "model-a".to_string(),
            provider_model_id: "model-a".to_string(),
            display_name: "Model A".to_string(),
            api_url_override: Some(
                "https://per-model.example.test/v1/chat/completions".to_string(),
            ),
            api_token_override: Some(format!("model-{secret}")),
            supports_image: false,
            context_window_tokens: Some(32_000),
            provider_profile_config: ProviderProfileConfig::generic_for_dialect(
                ProviderProtocolDialect::OpenAiChatCompletions,
            ),
            input_price: "0".to_string(),
            cached_input_price: "".to_string(),
            output_price: "0".to_string(),
            enabled: true,
        }],
    }
}

fn populated_storage(root: &Path, secret: &str) {
    let storage = open_test_storage(&root.join(DATABASE_FILE_NAME));
    storage
        .save_model_settings(model_settings_with_secret(secret))
        .unwrap();
    storage
        .set_skill_enablement_override("skill-a", false)
        .unwrap();
    let mut ui_preferences = storage.load_ui_preferences().unwrap();
    ui_preferences.profile_display_name = "Reset Test".to_string();
    ui_preferences.profile_handle = "RESET_CURRENT".to_string();
    ui_preferences.translucent_sidebar = true;
    ui_preferences.translucent_sidebar_transparency = 73;
    storage.save_ui_preferences(ui_preferences).unwrap();
    let mut prompt_preferences = storage.load_agent_prompt_preferences().unwrap();
    prompt_preferences.context_profile = mycopilot_core::AgentContextProfile::Minimal;
    prompt_preferences.custom_instructions = "Preserve this preference".to_string();
    storage
        .save_agent_prompt_preferences(prompt_preferences)
        .unwrap();
    let image_profile = ImageGenerationProfileRecord {
        id: DEFAULT_IMAGE_GENERATION_PROFILE_ID.to_string(),
        schema_version: IMAGE_GENERATION_PROFILE_SCHEMA_VERSION,
        adapter_id: "smartmlSeedream".to_string(),
        endpoint_url: "https://image.example.test/v1".to_string(),
        model_id: "image-model".to_string(),
        credential_ref: Some("opaque-credential-reference".to_string()),
        enabled: true,
        text_to_image: true,
        image_to_image: true,
        default_size_preset: "2K".to_string(),
        default_watermark: false,
        generation: 0,
        created_at: 0,
        updated_at: 0,
    };
    assert!(matches!(
        storage
            .compare_and_set_image_generation_profile(
                DEFAULT_IMAGE_GENERATION_PROFILE_ID,
                0,
                &image_profile,
            )
            .unwrap(),
        image_generation_repository::ImageGenerationProfileCompareAndSetOutcome::Updated(_)
    ));
    storage
        .save_project(ProjectRecord::with_primary_folder(
            "project-a".to_string(),
            "Disposable".to_string(),
            root.join("workspace").display().to_string(),
            1,
        ))
        .unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "disposable-conversation".to_string(),
            project_id: Some("project-a".to_string()),
            model_id: Some("model-a".to_string()),
            title: "Disposable conversation".to_string(),
            messages: vec![
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: "disposable-user-message".to_string(),
                    role: "user".to_string(),
                    content: "discard this conversation".to_string(),
                    created_at: 2,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: "disposable-assistant-message".to_string(),
                    role: "assistant".to_string(),
                    content: "discard this runtime".to_string(),
                    created_at: 3,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 2,
            updated_at: 3,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let trace = mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
        "disposable-run",
        "disposable-conversation",
        "disposable-assistant-message",
    );
    storage
        .append_in_progress_conversation_turn_trace(&trace, 3, 3)
        .unwrap();
    let download_directory = root.join("Downloads");
    fs::create_dir_all(&download_directory).unwrap();
    storage
        .save_browser_download_settings(BrowserDownloadSettingsUpdate {
            schema_version: BROWSER_DOWNLOAD_SCHEMA_VERSION,
            location_mode: BrowserDownloadLocationMode::Custom,
            custom_directory: Some(download_directory.display().to_string()),
            ask_where_to_save: true,
            expected_revision: 0,
            updated_at: 7,
        })
        .unwrap();
    let browser_preferences = storage.load_browser_preferences().unwrap();
    storage
        .save_browser_preferences(BrowserPreferencesUpdate {
            schema_version: BROWSER_DATA_SCHEMA_VERSION,
            link_open_target: BrowserLinkOpenTarget::Builtin,
            expected_revision: browser_preferences.revision,
            updated_at: 8,
        })
        .unwrap();
    let notification_settings = storage.load_notification_settings().unwrap();
    assert!(matches!(
        storage
            .update_notification_settings(&NotificationSettingsUpdate {
                enabled: false,
                sound_enabled: false,
                show_task_content: false,
                human_completed_enabled: false,
                human_failed_enabled: true,
                human_approval_enabled: false,
                human_cancelled_enabled: true,
                expected_revision: notification_settings.revision,
                updated_at: 9,
            })
            .unwrap(),
        NotificationSettingsUpdateOutcome::Updated(_)
    ));
    drop(storage);

    let connection = Connection::open(root.join(DATABASE_FILE_NAME)).unwrap();
    connection
        .execute(
            "INSERT INTO mcp_builtin_capability_metadata (
                     singleton, schema_version, revision_watermark
                 ) VALUES (1, 1, 1)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO mcp_builtin_capability_policies (
                     schema_version, capability_id, user_allowed,
                     policy_version, policy_revision
                 ) VALUES (1, 'browser_automation', 1, 1, 1)",
            [],
        )
        .unwrap();
    drop(connection);

    let registry = SqliteMcpRegistry::open(root.join(DATABASE_FILE_NAME)).unwrap();
    registry
        .add(McpServerConfig {
            id: McpServerId::new(),
            display_name: "Reset Test MCP".to_string(),
            scope: McpServerScope::User,
            trust: McpTrustLevel::Untrusted,
            approval_mode: McpApprovalMode::Prompt,
            enabled: false,
            transport: McpTransportConfig::Stdio(McpStdioConfig {
                program: std::env::current_exe().unwrap(),
                arguments: vec!["--reset-test".to_string()],
                cwd: root.to_path_buf(),
                environment: Vec::new(),
            }),
            connect_timeout_ms: 1_000,
            request_timeout_ms: 60_000,
            shutdown_timeout_ms: 2_000,
        })
        .unwrap();
}

#[test]
fn parser_requires_an_explicit_absolute_root() {
    let missing = parse_options(Vec::<OsString>::new()).unwrap_err();
    assert_eq!(missing.kind(), io::ErrorKind::InvalidInput);

    let parsed = parse_options([
        OsString::from(APP_DATA_ROOT_FLAG),
        OsString::from("/absolute/root"),
        OsString::from(CONFIRM_RESET_FLAG),
    ])
    .unwrap();
    assert!(parsed.confirm_reset);
    assert!(parsed.app_data_root.is_absolute());
    assert_eq!(parsed.configuration_source, None);
}

#[test]
fn parser_accepts_one_explicit_absolute_configuration_source() {
    let parsed = parse_options([
        OsString::from(APP_DATA_ROOT_FLAG),
        OsString::from("/absolute/root"),
        OsString::from(CONFIGURATION_SOURCE_FLAG),
        OsString::from("/absolute/root/storage-backups/source.sqlite"),
        OsString::from(CONFIRM_RESET_FLAG),
    ])
    .unwrap();
    assert_eq!(
        parsed.configuration_source,
        Some(PathBuf::from(
            "/absolute/root/storage-backups/source.sqlite"
        ))
    );
    assert!(parsed.confirm_reset);
}

#[test]
fn parser_rejects_invalid_configuration_source_arguments() {
    for arguments in [
        vec![
            OsString::from(APP_DATA_ROOT_FLAG),
            OsString::from("/absolute/root"),
            OsString::from(CONFIGURATION_SOURCE_FLAG),
        ],
        vec![
            OsString::from(APP_DATA_ROOT_FLAG),
            OsString::from("/absolute/root"),
            OsString::from(CONFIGURATION_SOURCE_FLAG),
            OsString::from("relative.sqlite"),
        ],
        vec![
            OsString::from(APP_DATA_ROOT_FLAG),
            OsString::from("/absolute/root"),
            OsString::from(CONFIGURATION_SOURCE_FLAG),
            OsString::from("/absolute/one.sqlite"),
            OsString::from(CONFIGURATION_SOURCE_FLAG),
            OsString::from("/absolute/two.sqlite"),
        ],
    ] {
        assert_eq!(
            parse_options(arguments).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}

#[test]
fn explicit_configuration_sources_are_pinned_to_known_catalogs_for_schema_49() {
    assert!(is_supported_explicit_configuration_source(
        FROZEN_WORKSPACE_CONFIGURATION_SOURCE_SCHEMA_VERSION,
        FROZEN_WORKSPACE_CONFIGURATION_SOURCE_FINGERPRINT,
    ));
    assert!(is_supported_explicit_configuration_source(
        RECOVERABLE_CONFIGURATION_SOURCE_SCHEMA_VERSION,
        RECOVERABLE_CONFIGURATION_SOURCE_FINGERPRINT,
    ));
    assert!(!is_supported_explicit_configuration_source(
        RECOVERABLE_CONFIGURATION_SOURCE_SCHEMA_VERSION - 1,
        RECOVERABLE_CONFIGURATION_SOURCE_FINGERPRINT,
    ));
    assert!(!is_supported_explicit_configuration_source(
        RECOVERABLE_CONFIGURATION_SOURCE_SCHEMA_VERSION,
        "sha256:tampered",
    ));
    assert!(is_supported_explicit_configuration_source(
        PREVIOUS_CONFIGURATION_SOURCE_SCHEMA_VERSION,
        PREVIOUS_CONFIGURATION_SOURCE_FINGERPRINT,
    ));
    assert!(is_supported_explicit_configuration_source(
        BLOCKING_INPUT_CONFIGURATION_SOURCE_SCHEMA_VERSION,
        BLOCKING_INPUT_CONFIGURATION_SOURCE_FINGERPRINT,
    ));
    assert!(!is_supported_explicit_configuration_source(
        35,
        "sha256:tampered"
    ));
    assert!(is_supported_explicit_configuration_source(
        COLLABORATION_CONFIGURATION_SOURCE_SCHEMA_VERSION,
        COLLABORATION_CONFIGURATION_SOURCE_FINGERPRINT,
    ));
    assert!(!is_supported_explicit_configuration_source(
        COLLABORATION_CONFIGURATION_SOURCE_SCHEMA_VERSION,
        "sha256:tampered",
    ));
    assert!(is_supported_explicit_configuration_source(
        CONTEXT_PROFILE_CONFIGURATION_SOURCE_SCHEMA_VERSION,
        CONTEXT_PROFILE_CONFIGURATION_SOURCE_FINGERPRINT,
    ));
    assert!(!is_supported_explicit_configuration_source(
        CONTEXT_PROFILE_CONFIGURATION_SOURCE_SCHEMA_VERSION,
        "sha256:tampered",
    ));
    assert!(!is_supported_explicit_configuration_source(
        mycopilot_core::storage::migrations::STORAGE_SCHEMA_VERSION,
        CONTEXT_PROFILE_CONFIGURATION_SOURCE_FINGERPRINT,
    ));
    let canonical = Connection::open_in_memory().unwrap();
    canonical
        .execute_batch(&canonical_schema_before_context_profiles())
        .unwrap();
    assert_eq!(
        storage_catalog_fingerprint(&canonical).unwrap(),
        COLLABORATION_CONFIGURATION_SOURCE_FINGERPRINT,
    );
    let context_profile = Connection::open_in_memory().unwrap();
    context_profile
        .execute_batch(&canonical_schema_before_multi_folder_projects())
        .unwrap();
    assert_eq!(
        storage_catalog_fingerprint(&context_profile).unwrap(),
        CONTEXT_PROFILE_CONFIGURATION_SOURCE_FINGERPRINT,
    );
}

#[test]
fn exact_v39_source_requires_its_pinned_catalog() {
    assert!(is_supported_explicit_configuration_source(
        39,
        REQUEST_WORLD_STATE_CONFIGURATION_SOURCE_FINGERPRINT
    ));
    assert!(!is_supported_explicit_configuration_source(
        38,
        REQUEST_WORLD_STATE_CONFIGURATION_SOURCE_FINGERPRINT
    ));
    assert!(!is_supported_explicit_configuration_source(
        39,
        "sha256:tampered"
    ));
}

#[test]
fn exact_v40_source_requires_its_pinned_catalog() {
    assert!(is_supported_explicit_configuration_source(
        40,
        IGNORED_HISTORY_CONFIGURATION_SOURCE_FINGERPRINT
    ));
    assert!(!is_supported_explicit_configuration_source(
        39,
        IGNORED_HISTORY_CONFIGURATION_SOURCE_FINGERPRINT
    ));
    assert!(!is_supported_explicit_configuration_source(
        40,
        "sha256:tampered"
    ));
}

#[test]
fn configuration_source_must_be_a_direct_regular_backup_file() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let backup_directory = root.join(BACKUP_DIRECTORY_NAME);
    create_private_directory(&backup_directory).unwrap();
    let source = backup_directory.join("source.sqlite");
    create_private_empty_file(&source).unwrap();
    assert_eq!(
        validate_configuration_source(root, &root.join(DATABASE_FILE_NAME), &source).unwrap(),
        fs::canonicalize(&source).unwrap()
    );

    let nested_directory = backup_directory.join("nested");
    create_private_directory(&nested_directory).unwrap();
    let nested = nested_directory.join("source.sqlite");
    create_private_empty_file(&nested).unwrap();
    assert_eq!(
        validate_configuration_source(root, &root.join(DATABASE_FILE_NAME), &nested)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );

    let outside = root.join("outside.sqlite");
    create_private_empty_file(&outside).unwrap();
    assert_eq!(
        validate_configuration_source(root, &root.join(DATABASE_FILE_NAME), &outside)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );

    #[cfg(unix)]
    {
        let link = backup_directory.join("source-link.sqlite");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        assert_eq!(
            validate_configuration_source(root, &root.join(DATABASE_FILE_NAME), &link)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}

#[test]
fn preserved_configuration_tables_must_match_the_current_schema_exactly() {
    let fixture = tempfile::tempdir().unwrap();
    let database = fixture.path().join(DATABASE_FILE_NAME);
    open_test_storage(&database);
    let connection = Connection::open(&database).unwrap();
    validate_preserved_configuration_table_schemas(&connection).unwrap();
    connection
        .execute("ALTER TABLE ui_preferences ADD COLUMN unexpected TEXT", [])
        .unwrap();
    assert_eq!(
        validate_preserved_configuration_table_schemas(&connection)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn confirmed_reset_initializes_an_absent_database_without_a_backup() {
    let fixture = tempfile::tempdir().unwrap();

    let report = execute(options(fixture.path(), true)).unwrap();

    assert!(report.confirmed);
    assert!(!report.source_existed);
    assert_eq!(report.backup_path, None);
    assert!(report.database_path.is_file());
    assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
    open_test_storage(&report.database_path);
    let connection = open_read_only(&report.database_path).unwrap();
    assert!(pragma_rows(&connection, "PRAGMA foreign_key_check")
        .unwrap()
        .is_empty());
    ensure_only_configuration_tables_have_rows(&connection).unwrap();
}

#[test]
fn reset_preserves_current_non_default_image_profiles_exactly() {
    let fixture = tempfile::tempdir().unwrap();
    let storage = open_test_storage(&fixture.path().join(DATABASE_FILE_NAME));
    let extra_profile = ImageGenerationProfileRecord {
        id: "future-profile".to_string(),
        schema_version: IMAGE_GENERATION_PROFILE_SCHEMA_VERSION,
        adapter_id: "smartmlSeedream".to_string(),
        endpoint_url: "https://image.example.test/v1".to_string(),
        model_id: "image-model".to_string(),
        credential_ref: None,
        enabled: true,
        text_to_image: true,
        image_to_image: false,
        default_size_preset: "2K".to_string(),
        default_watermark: true,
        generation: 0,
        created_at: 0,
        updated_at: 0,
    };
    assert!(matches!(
        storage
            .compare_and_set_image_generation_profile("future-profile", 0, &extra_profile,)
            .unwrap(),
        image_generation_repository::ImageGenerationProfileCompareAndSetOutcome::Updated(_)
    ));
    let expected = storage
        .load_image_generation_profile("future-profile")
        .unwrap()
        .unwrap();
    drop(storage);

    let report = execute(options(fixture.path(), true)).unwrap();

    assert_eq!(report.image_generation_profile_count, 1);
    let storage = open_test_storage(&fixture.path().join(DATABASE_FILE_NAME));
    assert_eq!(
        storage
            .load_image_generation_profile("future-profile")
            .unwrap()
            .as_ref(),
        Some(&expected)
    );
}

#[test]
fn dry_run_is_read_only_and_report_is_redacted() {
    let fixture = tempfile::tempdir().unwrap();
    let secret = format!(
        "never-print-this-{}",
        TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    populated_storage(fixture.path(), &secret);
    let database = fixture.path().join(DATABASE_FILE_NAME);
    let before = fs::read(&database).unwrap();

    let report = execute(options(fixture.path(), false)).unwrap();

    assert!(!report.confirmed);
    assert_eq!(report.model_count, 1);
    assert_eq!(report.mcp_server_count, 1);
    assert_eq!(fs::read(database).unwrap(), before);
    assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
    assert!(!report.render().contains(&secret));
}

#[test]
fn current_schema_reset_rejects_an_extra_schema_object() {
    let fixture = tempfile::tempdir().unwrap();
    populated_storage(fixture.path(), "current-schema-tamper-secret");
    let database = fixture.path().join(DATABASE_FILE_NAME);
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch("CREATE TABLE injected_reset_table(value TEXT NOT NULL);")
        .unwrap();
    drop(connection);

    let error = execute(options(fixture.path(), false)).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("exactly canonical"));
    assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
}

#[test]
fn current_schema_reset_rejects_foreign_key_corruption() {
    let fixture = tempfile::tempdir().unwrap();
    populated_storage(fixture.path(), "current-foreign-key-secret");
    let database = fixture.path().join(DATABASE_FILE_NAME);
    let connection = Connection::open(&database).unwrap();
    connection
        .pragma_update(None, "foreign_keys", false)
        .unwrap();
    connection
        .execute(
            "INSERT INTO attachments (
                     id, conversation_id, message_id, project_id, kind,
                     original_name, mime_type, size_bytes, storage_rel_path, created_at
                 ) VALUES (
                     'orphan-attachment', 'missing-conversation', 'missing-message', NULL,
                     'file', 'orphan.txt', 'text/plain', 1, 'orphan.txt', 0
                 )",
            [],
        )
        .unwrap();
    drop(connection);

    let error = execute(options(fixture.path(), false)).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("foreign_key_check"));
    assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
}

#[test]
fn reset_rejects_a_non_current_per_model_protocol_revision() {
    let fixture = tempfile::tempdir().unwrap();
    populated_storage(fixture.path(), "reset-invalid-revision-token");
    let database = fixture.path().join(DATABASE_FILE_NAME);
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE models SET provider_protocol_revision = 'provider-protocol-v1:not-a-uuid'",
            [],
        )
        .unwrap();
    drop(connection);

    let error = execute(options(fixture.path(), false)).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error
        .to_string()
        .contains("current Provider Protocol revision"));
    assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
}

#[test]
fn reset_fails_closed_when_a_configuration_table_has_unknown_columns() {
    let fixture = tempfile::tempdir().unwrap();
    populated_storage(fixture.path(), "reset-unknown-column-token");
    let database = fixture.path().join(DATABASE_FILE_NAME);
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "ALTER TABLE models ADD COLUMN future_provider_setting TEXT",
            [],
        )
        .unwrap();
    drop(connection);
    let source_digest = file_digest(&database).unwrap();

    let error = execute(options(fixture.path(), false)).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("exactly canonical"));
    assert_eq!(file_digest(&database).unwrap(), source_digest);
    assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
}

#[test]
fn confirmed_reset_failure_preserves_current_source_and_recovery_backup() {
    let fixture = tempfile::tempdir().unwrap();
    let secret = "confirmed-reset-failure-secret";
    populated_storage(fixture.path(), secret);
    let database = fixture.path().join(DATABASE_FILE_NAME);
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE models SET provider_protocol_revision = 'provider-protocol-v1:not-a-uuid'",
            [],
        )
        .unwrap();
    drop(connection);
    let source_digest = file_digest(&database).unwrap();

    let error = execute(options(fixture.path(), true)).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error
        .to_string()
        .contains("current Provider Protocol revision"));
    assert!(!error.to_string().contains(secret));
    assert_eq!(file_digest(&database).unwrap(), source_digest);

    let backup_directory = fixture.path().join(BACKUP_DIRECTORY_NAME);
    let backups = fs::read_dir(&backup_directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| {
                    name.starts_with("storage-reset-dev-") && name.ends_with(".sqlite")
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        backups.len(),
        1,
        "one immutable recovery snapshot must remain"
    );
    // A canonical database uses WAL mode. Exercise the actual recovery shape by restoring
    // the immutable snapshot to a writable database identity instead of mutating the only
    // backup merely to inspect it.
    let recovered_database = fixture.path().join("recovered-current.sqlite");
    fs::copy(&backups[0], &recovered_database).unwrap();
    let backup = Connection::open(&recovered_database).unwrap();
    mycopilot_core::storage::migrations::run_migrations(&backup).unwrap();
    assert_eq!(pragma_rows(&backup, "PRAGMA quick_check").unwrap(), ["ok"]);
    assert_eq!(
        backup
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        i64::from(mycopilot_core::storage::migrations::STORAGE_SCHEMA_VERSION)
    );
    let credential_ref = backup
        .query_row(
            "SELECT api_token_ref FROM model_provider_settings WHERE id = 'default'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert!(CredentialReference::parse(&credential_ref).is_ok());
    assert!(!fs::read(&recovered_database)
        .unwrap()
        .windows(secret.len())
        .any(|window| window == secret.as_bytes()));
    drop(backup);
    remove_database_files(&recovered_database);

    let temporary_files = fs::read_dir(fixture.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".storage.sqlite.reset-dev-"))
        .collect::<Vec<_>>();
    assert!(
        temporary_files.is_empty(),
        "failed reset leaked temporary database files: {temporary_files:?}"
    );
}

#[test]
fn confirmed_reset_backs_up_and_restores_only_configuration() {
    let fixture = tempfile::tempdir().unwrap();
    let secret = "reset-test-api-token";
    populated_storage(fixture.path(), secret);
    let source = Connection::open(fixture.path().join(DATABASE_FILE_NAME)).unwrap();
    source.execute("UPDATE human_interaction_settings SET enabled = 0, revision = 37, updated_at = 123456 WHERE singleton = 1", []).unwrap();
    source.execute("UPDATE agent_collaboration_settings SET enabled=0,revision=8,updated_at=123457 WHERE singleton=1", []).unwrap();
    source.execute("INSERT INTO agent_collaboration_run_policies(run_id,enabled,revision,updated_at) VALUES ('disposable-run',1,1,0)", []).unwrap();
    let exact_configuration_before = snapshot_exact_configuration_tables(&source).unwrap();
    drop(source);

    let report = execute(options(fixture.path(), true)).unwrap();

    assert!(report.confirmed);
    let backup = report.backup_path.as_ref().unwrap();
    assert!(backup.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let storage = open_test_storage(&fixture.path().join(DATABASE_FILE_NAME));
    let snapshot = storage.load_model_settings_snapshot().unwrap().unwrap();
    assert_eq!(snapshot.settings.api_token, secret);
    assert_eq!(snapshot.settings.search_mode, "tavily");
    assert_eq!(snapshot.settings.tavily_api_key, format!("search-{secret}"));
    assert_eq!(
        snapshot.settings.models[0].api_url_override.as_deref(),
        Some("https://per-model.example.test/v1/chat/completions")
    );
    assert_eq!(
        snapshot.settings.models[0].api_token_override.as_deref(),
        Some(format!("model-{secret}").as_str())
    );
    assert_eq!(
        snapshot.settings.models[0]
            .provider_profile_config
            .profile()
            .id,
        mycopilot_core::ProviderProfileId::GenericOpenAiChat
    );
    assert!(snapshot.provider_protocol_revisions["model-a"].starts_with("provider-protocol-v1:"));
    assert!(
        !storage
            .load_skill_enablement(&["skill-a".to_string()])
            .unwrap()["skill-a"]
    );
    assert_eq!(
        storage.load_ui_preferences().unwrap().profile_display_name,
        "Reset Test"
    );
    assert_eq!(
        storage
            .load_agent_prompt_preferences()
            .unwrap()
            .custom_instructions,
        "Preserve this preference"
    );
    assert_eq!(
        storage
            .load_agent_prompt_preferences()
            .unwrap()
            .context_profile,
        mycopilot_core::AgentContextProfile::Minimal,
    );
    let image_profile = storage
        .load_image_generation_profile(DEFAULT_IMAGE_GENERATION_PROFILE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(
        image_profile.credential_ref.as_deref(),
        Some("opaque-credential-reference")
    );
    drop(storage);
    let registry = SqliteMcpRegistry::open(fixture.path().join(DATABASE_FILE_NAME)).unwrap();
    let mcp_servers = registry.list().unwrap();
    assert_eq!(mcp_servers.len(), 1);
    assert_eq!(mcp_servers[0].config.display_name, "Reset Test MCP");
    drop(registry);
    let connection = open_read_only(&report.database_path).unwrap();
    assert_eq!(
        snapshot_exact_configuration_tables(&connection).unwrap(),
        exact_configuration_before
    );
    assert_eq!(
        load_human_interaction_settings_for_reset(&connection).unwrap(),
        mycopilot_core::human_interaction::HumanInteractionSettings {
            enabled: false,
            revision: 37,
            updated_at: 123456,
        }
    );
    assert_eq!(
        load_agent_collaboration_settings_for_reset(&connection).unwrap(),
        mycopilot_core::AgentCollaborationSettings {
            enabled: false,
            revision: 8,
            updated_at: 123457
        }
    );
    assert_eq!(
        count_rows_if_table_exists(&connection, "agent_collaboration_run_policies").unwrap(),
        0
    );
    assert_eq!(
        count_rows_if_table_exists(&connection, "agent_collaboration_wake_policies").unwrap(),
        0
    );
    assert_eq!(
        count_rows_if_table_exists(&connection, "projects").unwrap(),
        0
    );
    assert!(pragma_rows(&connection, "PRAGMA foreign_key_check")
        .unwrap()
        .is_empty());
    assert!(!report.render().contains(secret));
}

#[test]
fn confirmed_reset_preserves_a_current_image_credential_through_reconciliation() {
    let fixture = tempfile::tempdir().unwrap();
    let database = fixture.path().join(DATABASE_FILE_NAME);
    let storage = open_test_storage(&database);
    let credentials = Arc::new(
        DevelopmentFileCredentialStore::new(
            fixture
                .path()
                .join("image-generation-development-credentials-v1"),
        )
        .unwrap(),
    );
    let reference = credentials.new_reference();
    credentials
        .replace(
            &reference,
            CredentialSecret::new("temporary-reset-test-secret").unwrap(),
        )
        .unwrap();
    let profile = ImageGenerationProfileRecord {
        id: DEFAULT_IMAGE_GENERATION_PROFILE_ID.to_string(),
        schema_version: IMAGE_GENERATION_PROFILE_SCHEMA_VERSION,
        adapter_id: "smartmlSeedream".to_string(),
        endpoint_url: "https://image.example.test/v1".to_string(),
        model_id: "image-model".to_string(),
        credential_ref: Some(reference.as_str().to_string()),
        enabled: true,
        text_to_image: true,
        image_to_image: false,
        default_size_preset: "2K".to_string(),
        default_watermark: false,
        generation: 0,
        created_at: 0,
        updated_at: 0,
    };
    assert!(matches!(
        storage
            .compare_and_set_image_generation_profile(
                DEFAULT_IMAGE_GENERATION_PROFILE_ID,
                0,
                &profile,
            )
            .unwrap(),
        image_generation_repository::ImageGenerationProfileCompareAndSetOutcome::Updated(_)
    ));
    drop(storage);
    assert!(credentials.get(&reference).unwrap().is_some());

    execute(options(fixture.path(), true)).unwrap();

    let storage = Arc::new(open_test_storage(&database));
    let restored = storage
        .load_image_generation_profile(DEFAULT_IMAGE_GENERATION_PROFILE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(restored.credential_ref.as_deref(), Some(reference.as_str()));
    let configuration = ImageGenerationConfigurationService::new(
        Arc::clone(&storage),
        Arc::clone(&credentials) as Arc<dyn CredentialStore>,
    );
    configuration.reconcile_credentials().unwrap();
    assert!(credentials.get(&reference).unwrap().is_some());
}

#[test]
fn unsupported_schema_with_configuration_refuses_reset_without_losing_the_source() {
    let fixture = tempfile::tempdir().unwrap();
    populated_storage(fixture.path(), "v30-must-not-be-discarded-token");
    let database = fixture.path().join(DATABASE_FILE_NAME);
    let connection = Connection::open(&database).unwrap();
    connection.pragma_update(None, "user_version", 30).unwrap();
    drop(connection);
    let before = fs::read(&database).unwrap();
    for confirm in [false, true] {
        let error = execute(options(fixture.path(), confirm)).unwrap_err();
        assert!(error.to_string().contains("avoid discarding"));
        assert_eq!(fs::read(&database).unwrap(), before);
    }
}

#[test]
fn unsupported_empty_configuration_layout_is_not_treated_as_missing() {
    let fixture = tempfile::tempdir().unwrap();
    let database = fixture.path().join(DATABASE_FILE_NAME);
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE models(unknown_credential_layout BLOB); PRAGMA user_version = 30;",
        )
        .unwrap();
    drop(connection);
    let before = fs::read(&database).unwrap();
    assert!(execute(options(fixture.path(), true)).is_err());
    assert_eq!(fs::read(database).unwrap(), before);
}

fn drop_frozen_workspace_fixture_schema(connection: &Connection) {
    drop_local_token_fixture_schema(connection);
    connection
        .execute_batch(
            "DROP TABLE agent_workspace_run_bindings; DROP TABLE agent_workspace_wake_bindings;",
        )
        .unwrap();
}

fn restore_v47_search_fixture_schema(connection: &Connection) {
    remove_v60_fixture_objects(connection);
    connection
        .execute_batch(
            "DROP TRIGGER conversation_history_fts_message_insert;
             DROP TRIGGER conversation_history_fts_message_update;
             DROP TRIGGER conversation_history_fts_message_delete;
             DROP TRIGGER conversation_history_fts_trace_insert;
             DROP TRIGGER conversation_history_fts_trace_update;
             DROP TRIGGER conversation_history_fts_trace_delete;
             DROP TRIGGER conversation_history_fts_archive_delete;
             DROP TRIGGER conversation_history_index_entries_delete;
             DROP TABLE conversation_history_index_entries;",
        )
        .unwrap();
    connection
        .execute_batch(include_str!(
            "../../../../core/src/storage/test_fixtures/history_search_v47.sql"
        ))
        .unwrap();
}

fn drop_local_token_fixture_schema(connection: &Connection) {
    restore_v47_search_fixture_schema(connection);
    restore_legacy_guidance_content_schema(connection);
    connection.execute_batch("DROP TABLE local_token_usage_days; DROP TABLE local_token_usage_requests; DROP TABLE local_token_usage_metadata;").unwrap();
}

fn remove_v53_fixture_objects(connection: &Connection) {
    remove_v60_fixture_objects(connection);
    remove_workflow_fixture_objects(connection);
    assert_eq!(
        count_rows_if_table_exists(connection, "agent_collaboration_event_activities").unwrap(),
        0
    );
    connection
        .execute_batch(
            "DROP TRIGGER project_agent_collaboration_event_activities;
             DROP TRIGGER validate_agent_collaboration_event_activity_insert;
             DROP TRIGGER prevent_agent_collaboration_event_activity_update;
             DROP TRIGGER prevent_agent_collaboration_event_activity_delete;
             DROP VIEW agent_collaboration_task_activity_sources;
             DROP VIEW agent_collaboration_owner_placements;
             DROP TABLE agent_collaboration_event_activities;",
        )
        .unwrap();
}

// Test-only downgrade: the indexed history projection did not exist in the old recovery
// catalogs. Some downgrade paths call this twice, before removing the FTS identity table
// and again before stripping workflow fixtures.
fn remove_v60_fixture_objects(connection: &Connection) {
    let has_timeline: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema
                 WHERE name = 'conversation_history_timeline')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    if !has_timeline {
        return;
    }
    connection
        .execute_batch(
            "DROP TRIGGER message_presentation_message_update;
                 DROP TRIGGER message_presentation_ui_insert;
                 DROP TRIGGER message_presentation_ui_update;
                 DROP TRIGGER message_presentation_ui_delete;
                 DROP INDEX idx_messages_presentation_revision;
                 ALTER TABLE messages DROP COLUMN presentation_revision;
                 DROP TRIGGER message_history_conversation_insert;
                 DROP TRIGGER message_history_message_insert;
                 DROP TRIGGER message_history_message_update;
                 DROP TRIGGER message_history_message_delete;
                 DROP TABLE conversation_message_history_revisions;
                 DROP TRIGGER trace_journal_header_insert;
                 DROP TRIGGER trace_journal_header_update;
                 DROP TRIGGER trace_journal_header_delete;
                 DROP TRIGGER trace_journal_item_insert;
                 DROP TRIGGER trace_journal_item_update;
                 DROP TRIGGER trace_journal_item_delete;
                 DROP TRIGGER trace_journal_model_insert;
                 DROP TRIGGER trace_journal_model_update;
                 DROP TRIGGER trace_journal_model_delete;
                 DROP TABLE conversation_trace_journal_revisions;
                 DROP INDEX idx_human_interaction_requests_attention;
                 ALTER TABLE attachments DROP COLUMN pasted_text_json;
                 DROP TRIGGER IF EXISTS conversation_history_timeline_message_insert;
                 DROP TRIGGER IF EXISTS conversation_history_timeline_trace_insert;
                 DROP TABLE IF EXISTS conversation_history_timeline;
                 DROP INDEX IF EXISTS idx_messages_pending_assistant_summary;",
        )
        .unwrap();
    // The seeded fixture has no organization/workflow rows. Rebuild that empty catalog
    // exactly as v59, including the graph tables retired by v66 and pre-v65 identities.
    remove_workflow_fixture_objects(connection);
    let workflow_schema = include_str!("../../../../core/src/storage/canonical_schema.sql")
            .split_once("-- Workflow authoring definitions, schema v54. Runtime execution is not stored here.\n")
            .unwrap()
            .1
            .split_once("-- Indexed chronological history projection, schema v60.")
            .unwrap()
            .0;
    connection.execute_batch(workflow_schema).unwrap();
}

fn remove_workflow_fixture_objects(connection: &Connection) {
    let tables = connection
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table'
                 AND (name GLOB 'workflow_*' OR name = 'organization_personnel_receipts')",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER IF EXISTS workflow_bound_conversation_before_delete;
                 DROP TRIGGER IF EXISTS workflow_bound_conversation_archive_changed;
                 DROP TRIGGER IF EXISTS workflow_active_conversation_archive_guard;",
        )
        .unwrap();
    for table in tables {
        assert_eq!(
            count_rows_if_table_exists(connection, &table).unwrap(),
            0,
            "historical reset fixture must not contain workflow data: {table}"
        );
        connection
            .execute_batch(&format!("DROP TABLE \"{}\";", table.replace('"', "\"\"")))
            .unwrap();
    }
}

/// Restore only temporary reset fixtures. They have no request-owned activities or
/// attachment-only guidance; the reset tool's supported recovery catalogs stay unchanged.
fn restore_legacy_guidance_content_schema(connection: &Connection) {
    remove_v53_fixture_objects(connection);
    let legacy = Connection::open_in_memory().unwrap();
    legacy
        .execute_batch(&canonical_schema_v47())
        .expect("legacy guidance fixture schema");
    let guidance_sql: String = legacy
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='agent_run_guidances'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let index_sql = [
        "idx_agent_run_guidances_run_status",
        "idx_agent_run_guidances_conversation",
    ]
    .into_iter()
    .map(|name| {
        legacy
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='index' AND name=?1",
                [name],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    })
    .collect::<Vec<_>>();
    let trigger_sql = legacy
        .prepare(
            "SELECT sql FROM sqlite_schema
                 WHERE type='trigger' AND tbl_name='agent_run_guidances' AND sql IS NOT NULL
                 ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();

    connection
        .execute_batch(
            "PRAGMA foreign_keys=OFF;
                 CREATE TABLE reset_legacy_guidance_rows AS
                   SELECT guidance_id, client_message_id, run_id, conversation_id,
                          assistant_message_id, content, status, applied_trace_sequence,
                          terminal_reason, created_at, updated_at
                   FROM agent_run_guidances;
                 DROP TRIGGER human_interaction_async_guidance_proof;
                 DROP TRIGGER human_interaction_async_guidance_applied;
                 DROP TRIGGER human_interaction_async_guidance_unconsumed;
                 DROP INDEX idx_agent_run_guidances_run_status;
                 DROP INDEX idx_agent_run_guidances_conversation;
                 DROP TABLE agent_run_guidances;",
        )
        .unwrap();
    connection.execute_batch(&guidance_sql).unwrap();
    connection
        .execute_batch(
            "INSERT INTO agent_run_guidances(
                     guidance_id, client_message_id, run_id, conversation_id,
                     assistant_message_id, content, status, applied_trace_sequence,
                     terminal_reason, created_at, updated_at
                 )
                 SELECT guidance_id, client_message_id, run_id, conversation_id,
                        assistant_message_id, content, status, applied_trace_sequence,
                        terminal_reason, created_at, updated_at
                 FROM reset_legacy_guidance_rows;
                 DROP TABLE reset_legacy_guidance_rows;",
        )
        .unwrap();
    for sql in index_sql.into_iter().chain(trigger_sql) {
        connection.execute_batch(&sql).unwrap();
    }
    connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
}

#[test]
fn exact_v46_preview_and_refused_normal_startup_leave_configuration_and_chats_unchanged() {
    let fixture = tempfile::tempdir().unwrap();
    let secret = "v46-reset-refusal-test-secret";
    populated_storage(fixture.path(), secret);
    let database = fixture.path().join(DATABASE_FILE_NAME);
    let connection = Connection::open(&database).unwrap();
    drop_local_token_fixture_schema(&connection);
    connection.pragma_update(None, "user_version", 46).unwrap();
    assert_eq!(
        storage_catalog_fingerprint(&connection).unwrap(),
        FROZEN_WORKSPACE_CONFIGURATION_SOURCE_FINGERPRINT
    );
    let settings_before = snapshot_exact_configuration_tables(&connection).unwrap();
    let conversations_before = count_rows_if_table_exists(&connection, "conversations").unwrap();
    assert!(conversations_before > 0);
    drop(connection);
    let before = fs::read(&database).unwrap();
    let preview = execute(options(fixture.path(), false)).unwrap();
    assert!(preview.preserved_configuration);
    assert_eq!(fs::read(&database).unwrap(), before);
    // Normal startup must not upgrade or reset a development-era database.
    let error = mycopilot_core::storage::StorageState::open(&database)
        .err()
        .expect("v46 normal startup must require an explicit development reset");
    assert!(error
        .to_string()
        .contains(mycopilot_core::storage::migrations::DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
    assert_eq!(fs::read(&database).unwrap(), before);
    // The production reset entry canonicalizes the root before its NOFOLLOW open; macOS
    // tempfile paths use the /var -> /private/var alias and need the same normalization.
    let unchanged = open_read_only(&fs::canonicalize(&database).unwrap()).unwrap();
    assert_eq!(storage_schema_version(&unchanged).unwrap(), 46);
    assert_eq!(
        snapshot_exact_configuration_tables(&unchanged).unwrap(),
        settings_before
    );
    assert_eq!(
        count_rows_if_table_exists(&unchanged, "conversations").unwrap(),
        conversations_before
    );
}

fn downgrade_fixture_to_exact_v45(database: &Path) {
    let connection = Connection::open(database).unwrap();
    drop_frozen_workspace_fixture_schema(&connection);
    connection.pragma_update(None, "user_version", 45).unwrap();
    assert_eq!(
        storage_catalog_fingerprint(&connection).unwrap(),
        WORKSPACE_CONFIGURATION_SOURCE_FINGERPRINT
    );
}

fn drop_multi_folder_project_fixture_schema(connection: &Connection) {
    drop_frozen_workspace_fixture_schema(connection);
    // Only temporary fixture databases are rewritten. Recreate the pre-v45 projects table
    // byte-for-byte and fold each primary folder back into the legacy path column. Dropping
    // the table also drops its index and trigger, so their catalog SQL is replayed verbatim.
    let dependent_sql = connection
        .prepare(
            "SELECT sql FROM sqlite_schema
                 WHERE tbl_name = 'projects' AND type IN ('index', 'trigger') AND sql IS NOT NULL
                 ORDER BY type, name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(dependent_sql.len(), 2, "projects index and delete trigger");
    connection
        .execute_batch(
            "PRAGMA foreign_keys=OFF;
                 CREATE TABLE reset_old_projects AS
                   SELECT p.id, p.name,
                          (SELECT f.path FROM project_folders f
                            WHERE f.project_id = p.id AND f.role = 'primary') AS path,
                          p.created_at, p.pinned_at, p.updated_at
                   FROM projects p;
                 DROP INDEX project_folders_primary_per_project;
                 DROP INDEX project_folders_project_order;
                 DROP TABLE project_folders;
                 DROP TABLE projects;",
        )
        .unwrap();
    connection
        .execute_batch(&format!(
            "{PROJECTS_TABLE_SQL_BEFORE_V45}
                 INSERT INTO projects(id, name, path, created_at, pinned_at, updated_at)
                   SELECT id, name, path, created_at, pinned_at, updated_at
                   FROM reset_old_projects;
                 DROP TABLE reset_old_projects;"
        ))
        .unwrap();
    for sql in dependent_sql {
        connection.execute_batch(&sql).unwrap();
    }
}

fn downgrade_fixture_to_exact_v44(database: &Path) {
    let connection = Connection::open(database).unwrap();
    drop_multi_folder_project_fixture_schema(&connection);
    connection.pragma_update(None, "user_version", 44).unwrap();
    assert_eq!(
        storage_catalog_fingerprint(&connection).unwrap(),
        CONTEXT_PROFILE_CONFIGURATION_SOURCE_FINGERPRINT,
    );
}

fn drop_context_profile_fixture_schema(connection: &Connection) {
    drop_multi_folder_project_fixture_schema(connection);
    // Recreate the pre-v44 preference table SQL byte-for-byte; ALTER DROP COLUMN would
    // leave a different catalog representation.
    let old_schema = canonical_schema_before_context_profiles();
    let old_preferences_sql = old_schema
        .split_once("CREATE TABLE agent_prompt_preferences (")
        .unwrap()
        .1
        .split_once(");")
        .unwrap()
        .0;
    connection
        .execute_batch(
            "PRAGMA foreign_keys=OFF;
            DROP TABLE agent_context_profile_run_policies;
            DROP TABLE agent_context_profile_wake_policies;
            CREATE TABLE reset_old_prompt_preferences AS
              SELECT id, work_mode, tone, detail_level, custom_instructions, updated_at
              FROM agent_prompt_preferences;
            DROP TABLE agent_prompt_preferences;",
        )
        .unwrap();
    connection
        .execute_batch(&format!(
            "CREATE TABLE agent_prompt_preferences ({old_preferences_sql});
             INSERT INTO agent_prompt_preferences SELECT * FROM reset_old_prompt_preferences;
             DROP TABLE reset_old_prompt_preferences;"
        ))
        .unwrap();
}

fn downgrade_fixture_to_exact_v43(database: &Path) {
    let connection = Connection::open(database).unwrap();
    drop_context_profile_fixture_schema(&connection);
    connection.pragma_update(None, "user_version", 43).unwrap();
    assert_eq!(
        storage_catalog_fingerprint(&connection).unwrap(),
        COLLABORATION_CONFIGURATION_SOURCE_FINGERPRINT,
    );
}

fn drop_collaboration_policy_fixture_schema(connection: &Connection) {
    drop_context_profile_fixture_schema(connection);
    connection.execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE agent_collaboration_run_policies; DROP TABLE agent_collaboration_wake_policies; DROP TABLE agent_collaboration_settings;").unwrap();
}

fn downgrade_fixture_to_exact_v42(database: &Path) {
    let connection = Connection::open(database).unwrap();
    drop_collaboration_policy_fixture_schema(&connection);
    connection.pragma_update(None, "user_version", 42).unwrap();
    assert_eq!(
        storage_catalog_fingerprint(&connection).unwrap(),
        TRACE_PREFIX_CONFIGURATION_SOURCE_FINGERPRINT
    );
}

fn drop_ignored_projection_fixture_schema(connection: &Connection) {
    drop_collaboration_policy_fixture_schema(connection);
    // The seeded historical fixtures have an empty trace. Recreate its pinned table,
    // index and FTS triggers so v40 and earlier catalogs do not inherit v41's item kind.
    assert_eq!(
        count_rows_if_table_exists(connection, "conversation_turn_trace_items").unwrap(),
        0
    );
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
                 DROP TRIGGER human_interaction_ignored_projection_target_deleted;
                 DROP TABLE human_interaction_ignored_projections;
                 DROP TABLE conversation_turn_trace_items;",
        )
        .unwrap();
    connection
        .execute_batch(include_str!("../../../tests/fixtures/trace_items_v40.sql"))
        .unwrap();
}

fn downgrade_fixture_to_exact_v41(database: &Path) {
    let connection = Connection::open(database).unwrap();
    drop_collaboration_policy_fixture_schema(&connection);
    assert_eq!(
        count_rows_if_table_exists(&connection, "conversation_turn_trace_items").unwrap(),
        0
    );
    connection
        .execute_batch("PRAGMA foreign_keys = OFF; DROP TABLE conversation_turn_trace_items;")
        .unwrap();
    connection
        .execute_batch(include_str!("../../../tests/fixtures/trace_items_v41.sql"))
        .unwrap();
    connection.pragma_update(None, "user_version", 41).unwrap();
    assert_eq!(
        storage_catalog_fingerprint(&connection).unwrap(),
        UNIFIED_HISTORY_CONFIGURATION_SOURCE_FINGERPRINT
    );
}

fn downgrade_fixture_to_exact_v40(database: &Path) {
    let connection = Connection::open(database).unwrap();
    drop_ignored_projection_fixture_schema(&connection);
    connection.pragma_update(None, "user_version", 40).unwrap();
    assert_eq!(
        storage_catalog_fingerprint(&connection).unwrap(),
        IGNORED_HISTORY_CONFIGURATION_SOURCE_FINGERPRINT
    );
}

fn drop_request_world_state_fixture_schema(connection: &Connection) {
    drop_ignored_projection_fixture_schema(connection);
    // This compatibility fixture is a temporary test database. Pin the historical table SQL
    // byte-for-byte so old-schema fingerprints cannot accidentally follow the new schema.
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             DROP TABLE conversation_world_state_request_commits;
             DROP TABLE conversation_world_state_records;",
        )
        .unwrap();
    connection
        .execute_batch(include_str!(
            "../../../tests/fixtures/world_state_records_v39.sql"
        ))
        .unwrap();
}

fn downgrade_fixture_to_exact_v39(database: &Path) {
    let connection = Connection::open(database).unwrap();
    drop_request_world_state_fixture_schema(&connection);
    connection.pragma_update(None, "user_version", 39).unwrap();
    assert_eq!(
        storage_catalog_fingerprint(&connection).unwrap(),
        REQUEST_WORLD_STATE_CONFIGURATION_SOURCE_FINGERPRINT
    );
}

fn drop_answer_projection_fixture_schema(connection: &Connection) {
    drop_request_world_state_fixture_schema(connection);
    connection.execute_batch("DROP TRIGGER human_interaction_message_projection_message_immutable; DROP TABLE human_interaction_message_projections;").unwrap();
}

fn downgrade_fixture_to_exact_v38(database: &Path) {
    let connection = Connection::open(database).unwrap();
    drop_answer_projection_fixture_schema(&connection);
    connection.pragma_update(None, "user_version", 38).unwrap();
    assert_eq!(
        storage_catalog_fingerprint(&connection).unwrap(),
        ASYNC_INPUT_CONFIGURATION_SOURCE_FINGERPRINT
    );
}

fn drop_async_fixture_schema(connection: &Connection) {
    drop_answer_projection_fixture_schema(connection);
    connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    let names = connection.prepare("SELECT name FROM sqlite_schema WHERE type='trigger' AND name LIKE 'human_interaction_async_%'").unwrap().query_map([], |r| r.get::<_, String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    for name in names {
        connection
            .execute_batch(&format!("DROP TRIGGER {name}"))
            .unwrap();
    }
    connection
        .execute_batch("DROP TABLE human_interaction_async_bindings")
        .unwrap();
}

fn downgrade_fixture_to_exact_v37(database: &Path) {
    let connection = Connection::open(database).unwrap();
    drop_async_fixture_schema(&connection);
    connection.pragma_update(None, "user_version", 37).unwrap();
    assert_eq!(
        storage_catalog_fingerprint(&connection).unwrap(),
        SYNC_INPUT_CONFIGURATION_SOURCE_FINGERPRINT
    );
}

fn assert_previous_reset_preserves_configuration(source_version: i32) {
    let fixture = tempfile::tempdir().unwrap();
    let secret = "world-state-v39-reset-test-token";
    populated_storage(fixture.path(), secret);
    let database = fs::canonicalize(fixture.path().join(DATABASE_FILE_NAME)).unwrap();
    let expected_fingerprint = match source_version {
        47 => {
            let connection = Connection::open(&database).unwrap();
            restore_v47_search_fixture_schema(&connection);
            restore_legacy_guidance_content_schema(&connection);
            connection.pragma_update(None, "user_version", 47).unwrap();
            LOCAL_TOKEN_CONFIGURATION_SOURCE_FINGERPRINT
        }
        39 => {
            downgrade_fixture_to_exact_v39(&database);
            REQUEST_WORLD_STATE_CONFIGURATION_SOURCE_FINGERPRINT
        }
        40 => {
            downgrade_fixture_to_exact_v40(&database);
            IGNORED_HISTORY_CONFIGURATION_SOURCE_FINGERPRINT
        }
        41 => {
            downgrade_fixture_to_exact_v41(&database);
            UNIFIED_HISTORY_CONFIGURATION_SOURCE_FINGERPRINT
        }
        42 => {
            downgrade_fixture_to_exact_v42(&database);
            TRACE_PREFIX_CONFIGURATION_SOURCE_FINGERPRINT
        }
        43 => {
            downgrade_fixture_to_exact_v43(&database);
            COLLABORATION_CONFIGURATION_SOURCE_FINGERPRINT
        }
        44 => {
            downgrade_fixture_to_exact_v44(&database);
            CONTEXT_PROFILE_CONFIGURATION_SOURCE_FINGERPRINT
        }
        45 => {
            downgrade_fixture_to_exact_v45(&database);
            WORKSPACE_CONFIGURATION_SOURCE_FINGERPRINT
        }
        46 => {
            let connection = Connection::open(&database).unwrap();
            drop_local_token_fixture_schema(&connection);
            connection.pragma_update(None, "user_version", 46).unwrap();
            FROZEN_WORKSPACE_CONFIGURATION_SOURCE_FINGERPRINT
        }
        _ => panic!("unsupported reset fixture"),
    };
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE human_interaction_settings SET enabled=0,revision=23,updated_at=456",
            [],
        )
        .unwrap();
    let expected_human = load_human_interaction_settings_for_reset(&connection).unwrap();
    let expected_collaboration = if source_version >= 43 {
        connection
            .execute(
                "UPDATE agent_collaboration_settings SET enabled=0,revision=17,updated_at=123",
                [],
            )
            .unwrap();
        Some(load_agent_collaboration_settings_for_reset(&connection).unwrap())
    } else {
        None
    };
    if source_version >= 44 {
        connection
            .execute(
                "UPDATE agent_prompt_preferences SET context_profile='minimal'",
                [],
            )
            .unwrap();
    }
    let exact_before = snapshot_exact_configuration_tables(&connection).unwrap();
    let old_conversations = count_rows_if_table_exists(&connection, "conversations").unwrap();
    assert!(old_conversations > 0);
    let old_projects = count_rows_if_table_exists(&connection, "projects").unwrap();
    assert!(old_projects > 0);
    validate_explicit_configuration_source_schema(&connection, source_version).unwrap();
    drop(connection);
    let before = fs::read(&database).unwrap();
    if source_version < LOCAL_TOKEN_CONFIGURATION_SOURCE_SCHEMA_VERSION {
        assert!(mycopilot_core::storage::migrations::run_migrations(
            &Connection::open(&database).unwrap()
        )
        .is_err());
    }
    // v47 has a separate, non-destructive startup upgrade. This test exercises an explicit
    // reset of the untouched v47 source, including its original private backup.
    assert_eq!(fs::read(&database).unwrap(), before);
    let preview = execute(options(fixture.path(), false)).unwrap();
    assert!(preview.preserved_configuration);
    assert_eq!(fs::read(&database).unwrap(), before);

    let report = execute(options(fixture.path(), true)).unwrap();
    let storage = open_test_storage(&database);
    let settings = storage
        .load_model_settings_snapshot()
        .unwrap()
        .unwrap()
        .settings;
    assert_eq!(settings.api_token, secret);
    assert_eq!(settings.tavily_api_key, format!("search-{secret}"));
    assert_eq!(
        settings.models[0].api_token_override.as_deref(),
        Some(format!("model-{secret}").as_str())
    );
    assert_eq!(
        storage.load_human_interaction_settings().unwrap(),
        expected_human
    );
    let preferences = storage.load_agent_prompt_preferences().unwrap();
    assert_eq!(
        preferences.context_profile,
        if source_version >= 44 {
            mycopilot_core::AgentContextProfile::Minimal
        } else {
            mycopilot_core::AgentContextProfile::Full
        }
    );
    assert_eq!(preferences.custom_instructions, "Preserve this preference");
    if let Some(expected) = expected_collaboration {
        assert_eq!(
            storage.load_agent_collaboration_settings().unwrap(),
            expected
        );
    }
    assert!(storage.load_conversations().unwrap().is_empty());
    assert!(
        storage.load_projects().unwrap().is_empty(),
        "single-path projects are discarded instead of migrated"
    );
    drop(storage);
    let current = open_read_only(&database).unwrap();
    assert_eq!(
        storage_schema_version(&current).unwrap(),
        mycopilot_core::storage::migrations::STORAGE_SCHEMA_VERSION
    );
    assert_eq!(
        snapshot_exact_configuration_tables(&current).unwrap(),
        exact_before
    );
    assert_eq!(
        count_rows_if_table_exists(&current, "conversation_world_state_request_commits").unwrap(),
        0
    );
    for table in [
        "agent_context_profile_run_policies",
        "agent_context_profile_wake_policies",
    ] {
        assert_eq!(count_rows_if_table_exists(&current, table).unwrap(), 0);
    }
    let backup = open_read_only(report.backup_path.as_ref().unwrap()).unwrap();
    assert_eq!(storage_schema_version(&backup).unwrap(), source_version);
    assert_eq!(
        storage_catalog_fingerprint(&backup).unwrap(),
        expected_fingerprint
    );
    assert_eq!(
        count_rows_if_table_exists(&backup, "conversations").unwrap(),
        old_conversations
    );
    assert_eq!(
        load_human_interaction_settings_for_reset(&backup).unwrap(),
        expected_human
    );
}

#[test]
fn exact_v47_explicit_reset_preserves_configuration_and_backs_up_discarded_history() {
    assert_previous_reset_preserves_configuration(47);
}

#[test]
fn exact_v46_explicit_reset_preserves_configuration_and_backs_up_discarded_history() {
    assert_previous_reset_preserves_configuration(46);
}

#[test]
fn exact_v45_reset_preserves_configuration_and_discards_multi_folder_history() {
    assert_previous_reset_preserves_configuration(45);
}

#[test]
fn exact_v44_reset_preserves_context_profile_and_discards_single_path_projects() {
    assert_previous_reset_preserves_configuration(44);
}

#[test]
fn exact_v43_reset_preserves_collaboration_and_defaults_full_context_profile() {
    assert_previous_reset_preserves_configuration(43);
}

#[test]
fn exact_v42_reset_preserves_configuration_and_defaults_new_collaboration_setting() {
    assert_previous_reset_preserves_configuration(42);
}

#[test]
fn exact_v39_reset_preserves_configuration_credential_refs_and_human_revision() {
    assert_previous_reset_preserves_configuration(39);
}

#[test]
fn exact_v41_reset_preserves_configuration_credential_refs_and_human_revision() {
    assert_previous_reset_preserves_configuration(41);
}

#[test]
fn exact_v40_reset_preserves_configuration_credential_refs_and_human_revision() {
    assert_previous_reset_preserves_configuration(40);
}

#[test]
fn tampered_v39_reset_refuses_before_configuration_is_replaced() {
    let fixture = tempfile::tempdir().unwrap();
    populated_storage(fixture.path(), "v39-tampered-test-secret");
    let database = fixture.path().join(DATABASE_FILE_NAME);
    downgrade_fixture_to_exact_v39(&database);
    let connection = Connection::open(&database).unwrap();
    connection
        .execute("CREATE TABLE unexpected(value TEXT)", [])
        .unwrap();
    drop(connection);
    let before = fs::read(&database).unwrap();
    for confirmed in [false, true] {
        assert!(execute(options(fixture.path(), confirmed))
            .unwrap_err()
            .to_string()
            .contains("not exactly canonical"));
        assert_eq!(fs::read(&database).unwrap(), before);
    }
}

#[test]
fn exact_v37_reset_preserves_credentials_and_policy_without_history_migration() {
    let fixture = tempfile::tempdir().unwrap();
    let secret = "round-three-reset-test-token";
    populated_storage(fixture.path(), secret);
    let database = fs::canonicalize(fixture.path().join(DATABASE_FILE_NAME)).unwrap();
    downgrade_fixture_to_exact_v37(&database);
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE human_interaction_settings SET enabled=0,revision=9,updated_at=99",
            [],
        )
        .unwrap();
    let expected = load_human_interaction_settings_for_reset(&connection).unwrap();
    drop(connection);
    let bytes = fs::read(&database).unwrap();
    assert!(mycopilot_core::storage::migrations::run_migrations(
        &Connection::open(&database).unwrap()
    )
    .is_err());
    assert_eq!(fs::read(&database).unwrap(), bytes);
    execute(options(fixture.path(), true)).unwrap();
    let storage = open_test_storage(&database);
    let settings = storage
        .load_model_settings_snapshot()
        .unwrap()
        .unwrap()
        .settings;
    assert_eq!(settings.api_token, secret);
    assert_eq!(
        settings.models[0].api_token_override.as_deref(),
        Some(format!("model-{secret}").as_str())
    );
    assert_eq!(storage.load_human_interaction_settings().unwrap(), expected);
    assert!(storage.load_conversations().unwrap().is_empty());
}

#[test]
fn exact_v38_reset_preserves_credentials_and_policy_without_history_migration() {
    let fixture = tempfile::tempdir().unwrap();
    let secret = "round-five-reset-test-token";
    populated_storage(fixture.path(), secret);
    let database = fs::canonicalize(fixture.path().join(DATABASE_FILE_NAME)).unwrap();
    downgrade_fixture_to_exact_v38(&database);
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE human_interaction_settings SET enabled=0,revision=9,updated_at=99",
            [],
        )
        .unwrap();
    let expected = load_human_interaction_settings_for_reset(&connection).unwrap();
    drop(connection);
    let bytes = fs::read(&database).unwrap();
    assert!(mycopilot_core::storage::migrations::run_migrations(
        &Connection::open(&database).unwrap()
    )
    .is_err());
    assert_eq!(fs::read(&database).unwrap(), bytes);
    execute(options(fixture.path(), true)).unwrap();
    let storage = open_test_storage(&database);
    let settings = storage
        .load_model_settings_snapshot()
        .unwrap()
        .unwrap()
        .settings;
    assert_eq!(settings.api_token, secret);
    assert_eq!(
        settings.models[0].api_token_override.as_deref(),
        Some(format!("model-{secret}").as_str())
    );
    assert_eq!(storage.load_human_interaction_settings().unwrap(), expected);
    assert!(storage.load_conversations().unwrap().is_empty());
}

fn downgrade_fixture_to_exact_v36(database: &Path) {
    let connection = Connection::open(database).unwrap();
    drop_async_fixture_schema(&connection);
    let schema = include_str!("../../../../core/src/storage/canonical_schema.sql");
    let suspension = schema
        .split_once("CREATE TABLE human_interaction_suspensions (")
        .unwrap()
        .1
        .split_once("\n);")
        .unwrap()
        .0;
    let old = suspension.replace("status IN ('waiting', 'claimed', 'executing', 'model_in_flight', 'applied', 'failed', 'cancelled')", "status IN ('waiting', 'cancelled')").replace("\n    claim_id TEXT,", "");
    connection.execute_batch(&format!("PRAGMA foreign_keys=OFF; DROP TRIGGER human_interaction_sync_stop_fence; DROP TRIGGER human_interaction_deliveries_terminal; DROP TABLE human_interaction_suspensions; CREATE TABLE human_interaction_suspensions ({old}\n); PRAGMA user_version=36;")).unwrap();
    assert_eq!(
        storage_catalog_fingerprint(&connection).unwrap(),
        BLOCKING_INPUT_CONFIGURATION_SOURCE_FINGERPRINT
    );
}

#[test]
fn exact_v36_reset_preserves_model_credentials_and_human_policy_without_history() {
    let fixture = tempfile::tempdir().unwrap();
    let secret = "round-two-reset-test-token";
    populated_storage(fixture.path(), secret);
    let database = fs::canonicalize(fixture.path().join(DATABASE_FILE_NAME)).unwrap();
    downgrade_fixture_to_exact_v36(&database);
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE human_interaction_settings SET enabled=0,revision=7,updated_at=90",
            [],
        )
        .unwrap();
    let expected = load_human_interaction_settings_for_reset(&connection).unwrap();
    drop(connection);
    let bytes = fs::read(&database).unwrap();
    assert!(mycopilot_core::storage::migrations::run_migrations(
        &Connection::open(&database).unwrap()
    )
    .is_err());
    assert_eq!(fs::read(&database).unwrap(), bytes);
    execute(options(fixture.path(), true)).unwrap();
    let storage = open_test_storage(&database);
    let settings = storage
        .load_model_settings_snapshot()
        .unwrap()
        .unwrap()
        .settings;
    assert_eq!(settings.api_token, secret);
    assert_eq!(
        settings.models[0].api_token_override.as_deref(),
        Some(format!("model-{secret}").as_str())
    );
    assert!(storage.load_conversations().unwrap().is_empty());
    assert_eq!(storage.load_human_interaction_settings().unwrap(), expected);
}

#[test]
fn exact_v35_reset_preserves_configuration_and_credentials_without_migrating_history() {
    let fixture = tempfile::tempdir().unwrap();
    let secret = "previous-schema-reset-test-token";
    populated_storage(fixture.path(), secret);
    let database = fs::canonicalize(fixture.path().join(DATABASE_FILE_NAME)).unwrap();
    downgrade_fixture_to_exact_v35(&database);
    let before = fs::read(&database).unwrap();
    let connection = open_read_only(&database).unwrap();
    let exact_before = snapshot_exact_configuration_tables(&connection).unwrap();
    let old_conversations = count_rows_if_table_exists(&connection, "conversations").unwrap();
    assert!(old_conversations > 0);
    drop(connection);
    let preview = execute(options(fixture.path(), false)).unwrap();
    assert!(preview.preserved_configuration);
    assert_eq!(fs::read(&database).unwrap(), before);

    let report = execute(options(fixture.path(), true)).unwrap();
    let storage = open_test_storage(&database);
    let settings = storage
        .load_model_settings_snapshot()
        .unwrap()
        .unwrap()
        .settings;
    assert_eq!(settings.api_token, secret);
    assert_eq!(
        settings.models[0].api_token_override.as_deref(),
        Some(format!("model-{secret}").as_str())
    );
    assert_eq!(
        storage.load_ui_preferences().unwrap().profile_display_name,
        "Reset Test"
    );
    assert!(storage.load_conversations().unwrap().is_empty());
    drop(storage);
    let connection = open_read_only(&database).unwrap();
    assert_eq!(
        storage_schema_version(&connection).unwrap(),
        mycopilot_core::storage::migrations::STORAGE_SCHEMA_VERSION
    );
    assert_eq!(
        snapshot_exact_configuration_tables(&connection).unwrap(),
        exact_before
    );
    assert_eq!(
        load_human_interaction_settings_for_reset(&connection).unwrap(),
        Default::default()
    );
    let backup = open_read_only(report.backup_path.as_ref().unwrap()).unwrap();
    assert_eq!(storage_schema_version(&backup).unwrap(), 35);
    assert_eq!(
        count_rows_if_table_exists(&backup, "conversations").unwrap(),
        old_conversations
    );
    assert_eq!(
        storage_catalog_fingerprint(&backup).unwrap(),
        PREVIOUS_CONFIGURATION_SOURCE_FINGERPRINT
    );
}

#[test]
fn tampered_v35_reset_refuses_before_configuration_is_replaced() {
    let fixture = tempfile::tempdir().unwrap();
    populated_storage(fixture.path(), "preserve-tampered-previous-config");
    let database = fixture.path().join(DATABASE_FILE_NAME);
    downgrade_fixture_to_exact_v35(&database);
    let connection = Connection::open(&database).unwrap();
    connection
        .execute("CREATE TABLE unexpected(value TEXT)", [])
        .unwrap();
    drop(connection);
    let before = fs::read(&database).unwrap();
    assert!(execute(options(fixture.path(), true))
        .unwrap_err()
        .to_string()
        .contains("not exactly canonical"));
    assert_eq!(fs::read(database).unwrap(), before);
}

#[test]
fn reset_preserves_the_exact_mcp_model_namespace_after_a_display_name_change() {
    let fixture = tempfile::tempdir().unwrap();
    let database = fixture.path().join(DATABASE_FILE_NAME);
    open_test_storage(&database);
    let registry = SqliteMcpRegistry::open(&database).unwrap();
    let server_id = McpServerId::new();
    let original = registry
        .add_persisted(
            McpServerConfig {
                id: server_id,
                display_name: "Original Namespace Label".to_string(),
                scope: McpServerScope::User,
                trust: McpTrustLevel::Untrusted,
                approval_mode: McpApprovalMode::Prompt,
                enabled: false,
                transport: McpTransportConfig::Stdio(McpStdioConfig {
                    program: std::env::current_exe().unwrap(),
                    arguments: vec!["--namespace-reset-test".to_string()],
                    cwd: fixture.path().to_path_buf(),
                    environment: Vec::new(),
                }),
                connect_timeout_ms: 1_000,
                request_timeout_ms: 60_000,
                shutdown_timeout_ms: 2_000,
            },
            None,
        )
        .unwrap();
    let expected_namespace = original.model_namespace.clone();
    let mut renamed = original.config.clone();
    renamed.display_name = "Completely Different Label".to_string();
    registry
        .update_with_precondition(
            &McpRegistryMutationPrecondition::from_entry(&original),
            renamed,
        )
        .unwrap();
    assert_eq!(
        registry.get(server_id).unwrap().unwrap().model_namespace,
        expected_namespace
    );
    drop(registry);

    execute(options(fixture.path(), true)).unwrap();

    let restored = SqliteMcpRegistry::open(&database)
        .unwrap()
        .get(server_id)
        .unwrap()
        .unwrap();
    assert_eq!(restored.model_namespace, expected_namespace);
    assert_eq!(restored.config.display_name, "Completely Different Label");
}

#[test]
fn mcp_inspection_never_mutates_the_only_recovery_backup() {
    let fixture = tempfile::tempdir().unwrap();
    populated_storage(fixture.path(), "backup-secret");
    let database = fixture.path().join(DATABASE_FILE_NAME);
    let backup = create_database_backup(fixture.path(), &database).unwrap();
    let before = file_digest(&backup).unwrap();
    let working =
        TemporaryDatabaseSnapshot::from_source(fixture.path(), &backup, "mcp-test").unwrap();

    let records = load_mcp_records(working.path()).unwrap();

    assert_eq!(records.len(), 1);
    assert_eq!(file_digest(&backup).unwrap(), before);
}

#[test]
fn reset_refuses_a_live_database_owner() {
    let fixture = tempfile::tempdir().unwrap();
    populated_storage(fixture.path(), "secret");
    let database = fixture.path().join(DATABASE_FILE_NAME);
    let _lock = acquire_database_instance_lock(&database).unwrap();

    let error = execute(options(fixture.path(), true)).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
}

#[test]
fn reset_does_not_touch_another_storage_root() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    populated_storage(first.path(), "first-secret");
    populated_storage(second.path(), "second-secret");
    let second_before = fs::read(second.path().join(DATABASE_FILE_NAME)).unwrap();

    execute(options(first.path(), true)).unwrap();

    assert_eq!(
        fs::read(second.path().join(DATABASE_FILE_NAME)).unwrap(),
        second_before
    );
    assert!(!second.path().join(BACKUP_DIRECTORY_NAME).exists());
}

#[test]
fn reset_does_not_run_startup_cleanup_in_sibling_managed_directories() {
    let fixture = tempfile::tempdir().unwrap();
    populated_storage(fixture.path(), "filesystem-boundary-secret");
    let sentinels = [
        fixture.path().join("attachments/orphan/sentinel.bin"),
        fixture
            .path()
            .join("image-generation-artifacts/objects/sentinel.bin"),
        fixture.path().join("managed-command-runs/v1/sentinel.bin"),
        fixture.path().join("skills/sentinel/SKILL.md"),
        fixture.path().join("credentials/sentinel.bin"),
    ];
    for (index, path) in sentinels.iter().enumerate() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("managed-directory-sentinel-{index}")).unwrap();
    }
    let before = sentinels
        .iter()
        .map(|path| fs::read(path).unwrap())
        .collect::<Vec<_>>();

    execute(options(fixture.path(), true)).unwrap();

    for (path, expected) in sentinels.iter().zip(before) {
        assert_eq!(fs::read(path).unwrap(), expected);
    }
}
