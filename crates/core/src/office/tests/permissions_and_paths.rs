use super::*;

#[test]
fn mutation_destination_is_a_separate_cas_protected_publish_target() {
    let replacement = tempfile::NamedTempFile::new().unwrap();
    write_docx(replacement.path(), "replacement");
    let fixture = Fixture::new(&format!(
        "#!/bin/sh\n/bin/cp '{}' \"$2\"\n",
        replacement.path().display()
    ));
    let source = fixture.workspace.path().join("sample.docx");
    let destination = fixture.workspace.path().join("copy.docx");
    write_docx(&source, "source");
    let source_before = fs::read(&source).unwrap();
    let mut request = fixture.request(OfficeOperation::Set);
    request.destination_path = Some("copy.docx".to_string());
    let prepared = fixture
        .engine
        .prepare(&workspace_context(fixture.workspace.path()), &request)
        .unwrap();
    assert_eq!(
        prepared_path(&prepared, &OfficePathSlot::Document).state,
        OfficeFileState::Present
    );
    assert_eq!(
        prepared_path(&prepared, &OfficePathSlot::Destination).state,
        OfficeFileState::Missing
    );
    let result = fixture
        .engine
        .execute_prepared(
            &workspace_context(fixture.workspace.path()),
            &prepared,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert!(result.error_code.is_none(), "{:?}", result.error);
    assert_eq!(fs::read(&source).unwrap(), source_before);
    assert_eq!(
        fs::read(&destination).unwrap(),
        fs::read(replacement.path()).unwrap()
    );

    let second_destination = fixture.workspace.path().join("conflict.docx");
    let mut request = fixture.request(OfficeOperation::Set);
    request.destination_path = Some("conflict.docx".to_string());
    let prepared = fixture
        .engine
        .prepare(&workspace_context(fixture.workspace.path()), &request)
        .unwrap();
    write_docx(&second_destination, "concurrent-create");
    let concurrent = fs::read(&second_destination).unwrap();
    let error = fixture
        .engine
        .execute_prepared(
            &workspace_context(fixture.workspace.path()),
            &prepared,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::PreconditionFailed);
    assert_eq!(fs::read(second_destination).unwrap(), concurrent);

    let mut request = fixture.request(OfficeOperation::Set);
    request.destination_path = Some("source-changed.docx".to_string());
    let prepared = fixture
        .engine
        .prepare(&workspace_context(fixture.workspace.path()), &request)
        .unwrap();
    write_docx(&source, "concurrent-source-change");
    let error = fixture
        .engine
        .execute_prepared(
            &workspace_context(fixture.workspace.path()),
            &prepared,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::PreconditionFailed);
    assert!(!fixture
        .workspace
        .path()
        .join("source-changed.docx")
        .exists());
}

#[test]
fn destination_path_is_rejected_outside_mutation_operations() {
    let fixture = Fixture::new(basic_script());
    write_docx(&fixture.workspace.path().join("sample.docx"), "source");
    for operation in [
        OfficeOperation::Help,
        OfficeOperation::Create,
        OfficeOperation::View,
        OfficeOperation::Get,
        OfficeOperation::Query,
        OfficeOperation::Validate,
    ] {
        let mut request = fixture.request(operation);
        if operation == OfficeOperation::Help {
            request.document_path = None;
        }
        request.destination_path = Some("destination.docx".to_string());
        let error = fixture
            .engine
            .prepare(&workspace_context(fixture.workspace.path()), &request)
            .unwrap_err();
        assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);
    }
}

#[test]
fn cancellation_is_linearized_before_the_atomic_commit() {
    let replacement = tempfile::NamedTempFile::new().unwrap();
    write_docx(replacement.path(), "replacement");
    let fixture = Fixture::new(&format!(
        "#!/bin/sh\n/bin/cp '{}' \"$2\"\n",
        replacement.path().display()
    ));
    let target = fixture.workspace.path().join("sample.docx");
    write_docx(&target, "original");
    let original = fs::read(&target).unwrap();
    let hook_target = target.canonicalize().unwrap();

    let cancellation = Arc::new(AtomicBool::new(false));
    install_commit_test_hook(
        hook_target.clone(),
        CommitTestPhase::BeforeCancellationCheck,
        cancellation.clone(),
    );
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &fixture.request(OfficeOperation::Set),
            AgentCancellationToken::new(),
            Some(cancellation),
        )
        .unwrap();
    assert!(result.cancelled);
    assert_eq!(result.error_code.as_deref(), Some("office.cancelled"));
    assert_eq!(fs::read(&target).unwrap(), original);

    let cancellation = Arc::new(AtomicBool::new(false));
    install_commit_test_hook(
        hook_target,
        CommitTestPhase::AfterCancellationCheck,
        cancellation.clone(),
    );
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &fixture.request(OfficeOperation::Set),
            AgentCancellationToken::new(),
            Some(cancellation.clone()),
        )
        .unwrap();
    assert!(cancellation.load(Ordering::SeqCst));
    assert!(!result.cancelled);
    assert!(result.error_code.is_none(), "{:?}", result.error);
    assert_eq!(
        fs::read(target).unwrap(),
        fs::read(replacement.path()).unwrap()
    );
}

#[test]
fn symlinked_inputs_and_component_escapes_are_rejected() {
    let fixture = Fixture::new(basic_script());
    let outside = fixture.engine_dir.path().join("outside.docx");
    fs::write(&outside, b"secret").unwrap();
    symlink(&outside, fixture.workspace.path().join("sample.docx")).unwrap();
    let error = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &fixture.request(OfficeOperation::Validate),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);

    let resources = tempfile::tempdir().unwrap();
    let component = resources.path().join(office_cli_component_relative_path());
    fs::create_dir_all(component.parent().unwrap()).unwrap();
    symlink(fixture.engine.executable_path(), &component).unwrap();
    let options = OfficeCliDiscoveryOptions::new()
        .with_application_resources_dir(resources.path())
        .with_workspace_root(fixture.workspace.path());
    let error = OfficeCliEngine::discover(&options).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidConfiguration);
}

#[test]
fn rendering_requires_a_validated_output_path() {
    let fixture = Fixture::new(basic_script());
    fs::write(fixture.workspace.path().join("sample.docx"), b"doc").unwrap();
    let mut request = fixture.request(OfficeOperation::View);
    request.parameters = OfficeOperationParameters::View {
        mode: OfficeViewMode::Screenshot,
        start: None,
        end: None,
        max_lines: None,
        issue_type: None,
        limit: None,
        columns: Vec::new(),
        pages: Vec::new(),
        range: None,
        viewport: None,
        grid: None,
        render_mode: None,
        page_count: false,
    };
    let error = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);

    request.output_path = Some("preview.png".to_string());
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert_eq!(result.argv.last().map(String::as_str), Some("preview.png"));
    assert_eq!(request.access(), OfficeOperationAccess::FileWrite);
}

#[test]
fn missing_managed_browser_fails_before_officecli_starts() {
    let workspace = tempfile::tempdir().unwrap();
    let engine_directory = tempfile::tempdir().unwrap();
    let executable = engine_directory.path().join("officecli");
    let started_marker = workspace.path().join("officecli-started");
    write_executable(
        &executable,
        &format!("#!/bin/sh\ntouch '{}'\n", started_marker.display()),
    );
    let engine = OfficeCliEngine::discover(
        &OfficeCliDiscoveryOptions::new()
            .with_configured_executable(&executable)
            .with_browser_proxy_executable(&executable)
            .with_workspace_root(workspace.path()),
    )
    .unwrap();
    fs::write(workspace.path().join("sample.docx"), b"doc").unwrap();
    let request = OfficeExecutionRequest {
        document_kind: OfficeDocumentKind::Document,
        operation: OfficeOperation::View,
        document_path: Some("sample.docx".to_string()),
        parameters: OfficeOperationParameters::View {
            mode: OfficeViewMode::Screenshot,
            start: None,
            end: None,
            max_lines: None,
            issue_type: None,
            limit: None,
            columns: Vec::new(),
            pages: vec![OfficePageRange {
                start: 1,
                end: Some(3),
            }],
            range: None,
            viewport: None,
            grid: Some(OfficeGridLayout::Auto),
            render_mode: None,
            page_count: false,
        },
        output_path: Some("preview.png".to_string()),
        destination_path: None,
        inputs: Vec::new(),
        timeout_ms: Some(MAX_OFFICE_TIMEOUT_MS),
    };

    let started = Instant::now();
    let error = engine
        .prepare(&workspace_context(workspace.path()), &request)
        .unwrap_err();

    assert_eq!(
        error.code(),
        OfficeEngineErrorCode::RenderBackendUnavailable
    );
    assert_eq!(
        error.code().stable_name(),
        "office.render_backend_unavailable"
    );
    assert!(started.elapsed().as_millis() < 500);
    assert!(!started_marker.exists());
}

#[test]
fn browser_proxy_self_test_rejects_an_unidentified_executable_before_officecli_starts() {
    let workspace = tempfile::tempdir().unwrap();
    let engine_directory = tempfile::tempdir().unwrap();
    let executable = engine_directory.path().join("officecli");
    let started_marker = workspace.path().join("officecli-started");
    write_executable(
        &executable,
        &format!("#!/bin/sh\ntouch '{}'\n", started_marker.display()),
    );
    let proxy_directory = tempfile::tempdir().unwrap();
    let proxy = proxy_directory.path().join("core-server");
    write_executable(&proxy, "#!/bin/sh\nexit 0\n");
    let render_runtime_directory = tempfile::tempdir().unwrap();
    super::render_runtime::write_test_render_runtime(render_runtime_directory.path());
    let engine = OfficeCliEngine::discover(
        &OfficeCliDiscoveryOptions::new()
            .with_configured_executable(&executable)
            .with_configured_render_runtime_dir(render_runtime_directory.path())
            .with_browser_proxy_executable(&proxy)
            .with_workspace_root(workspace.path()),
    )
    .unwrap();
    fs::write(workspace.path().join("sample.docx"), b"doc").unwrap();
    let request = OfficeExecutionRequest {
        document_kind: OfficeDocumentKind::Document,
        operation: OfficeOperation::View,
        document_path: Some("sample.docx".to_string()),
        parameters: OfficeOperationParameters::View {
            mode: OfficeViewMode::Screenshot,
            start: None,
            end: None,
            max_lines: None,
            issue_type: None,
            limit: None,
            columns: Vec::new(),
            pages: vec![OfficePageRange {
                start: 1,
                end: Some(3),
            }],
            range: None,
            viewport: None,
            grid: Some(OfficeGridLayout::Auto),
            render_mode: None,
            page_count: false,
        },
        output_path: Some("preview.png".to_string()),
        destination_path: None,
        inputs: Vec::new(),
        timeout_ms: Some(MAX_OFFICE_TIMEOUT_MS),
    };

    let error = engine
        .execute(
            &workspace_context(workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();

    assert_eq!(
        error.code(),
        OfficeEngineErrorCode::RenderBackendUnavailable
    );
    assert!(error
        .message()
        .contains("did not identify itself as the trusted core-server proxy"));
    assert!(!started_marker.exists());
}

#[test]
fn office_paths_follow_the_read_write_permission_matrix() {
    let fixture = Fixture::new(basic_script());
    let workspace_document = fixture.workspace.path().join("sample.docx");
    write_docx(&workspace_document, "workspace");
    let external = tempfile::tempdir().unwrap();
    let external_root = external.path().canonicalize().unwrap();
    let external_document = external_root.join("external.docx");
    write_docx(&external_document, "external");

    let workspace_read = fixture.request(OfficeOperation::Validate);
    fixture
        .engine
        .prepare(
            &permission_context(
                Some(fixture.workspace.path()),
                AgentReadPermission::WorkspaceOnly,
                AgentWritePermission::Denied,
            ),
            &workspace_read,
        )
        .expect("write=denied must not remove Office read operations");

    let read = OfficeExecutionRequest {
        document_path: Some(external_document.to_string_lossy().into_owned()),
        ..fixture.request(OfficeOperation::Validate)
    };
    let error = fixture
        .engine
        .prepare(
            &permission_context(
                Some(fixture.workspace.path()),
                AgentReadPermission::WorkspaceOnly,
                AgentWritePermission::WorkspaceOnly,
            ),
            &read,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
    let prepared = fixture
        .engine
        .prepare(
            &permission_context(
                Some(fixture.workspace.path()),
                AgentReadPermission::All,
                AgentWritePermission::WorkspaceOnly,
            ),
            &read,
        )
        .unwrap();
    assert_eq!(
        prepared_path(&prepared, &OfficePathSlot::Document).scope,
        OfficePathScope::External
    );

    let create = OfficeExecutionRequest {
        document_kind: OfficeDocumentKind::Document,
        operation: OfficeOperation::Create,
        document_path: Some(
            external_root
                .join("created.docx")
                .to_string_lossy()
                .into_owned(),
        ),
        parameters: OfficeOperationParameters::Create {
            locale: None,
            minimal: false,
            overwrite: false,
        },
        output_path: None,
        destination_path: None,
        inputs: Vec::new(),
        timeout_ms: Some(10_000),
    };
    for write in [
        AgentWritePermission::Denied,
        AgentWritePermission::WorkspaceOnly,
    ] {
        let error = fixture
            .engine
            .prepare(
                &permission_context(
                    Some(fixture.workspace.path()),
                    AgentReadPermission::All,
                    write,
                ),
                &create,
            )
            .unwrap_err();
        assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
    }
    let prepared = fixture
        .engine
        .prepare(
            &permission_context(
                None,
                AgentReadPermission::WorkspaceOnly,
                AgentWritePermission::All,
            ),
            &create,
        )
        .unwrap();
    let target = prepared_path(&prepared, &OfficePathSlot::Document);
    assert_eq!(target.scope, OfficePathScope::External);
    assert_eq!(target.purpose, OfficePathPurpose::WriteTarget);
    assert_eq!(
        target.write_disposition,
        Some(OfficeWriteDisposition::CreateNew)
    );
    assert!(target.object_identity.is_none());

    let relative_without_workspace = OfficeExecutionRequest {
        document_path: Some("relative.docx".to_string()),
        ..create.clone()
    };
    let error = fixture
        .engine
        .prepare(
            &permission_context(None, AgentReadPermission::All, AgentWritePermission::All),
            &relative_without_workspace,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
}

#[test]
fn in_place_external_edits_use_write_permission_not_independent_read_permission() {
    let fixture = Fixture::new(basic_script());
    let external = tempfile::tempdir().unwrap();
    let document = external
        .path()
        .canonicalize()
        .unwrap()
        .join("external.docx");
    write_docx(&document, "external");
    let request = OfficeExecutionRequest {
        document_path: Some(document.to_string_lossy().into_owned()),
        ..fixture.request(OfficeOperation::Set)
    };
    let prepared = fixture
        .engine
        .prepare(
            &permission_context(
                Some(fixture.workspace.path()),
                AgentReadPermission::WorkspaceOnly,
                AgentWritePermission::All,
            ),
            &request,
        )
        .unwrap();
    let document = prepared_path(&prepared, &OfficePathSlot::Document);
    assert_eq!(document.purpose, OfficePathPurpose::InPlaceTarget);
    assert_eq!(document.scope, OfficePathScope::External);

    let mut save_as = request;
    save_as.destination_path = Some("copy.docx".to_string());
    let error = fixture
        .engine
        .prepare(
            &permission_context(
                Some(fixture.workspace.path()),
                AgentReadPermission::WorkspaceOnly,
                AgentWritePermission::All,
            ),
            &save_as,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
    fixture
        .engine
        .prepare(
            &permission_context(
                Some(fixture.workspace.path()),
                AgentReadPermission::All,
                AgentWritePermission::WorkspaceOnly,
            ),
            &save_as,
        )
        .unwrap();
}

#[test]
fn registered_attachments_are_read_only_sources() {
    let fixture = Fixture::new(basic_script());
    let library_root = tempfile::tempdir().unwrap();
    let stored = library_root.path().join("stored.docx");
    write_docx(&stored, "attachment");
    let read_path = "@attachments/a1/source.docx".to_string();
    let reference = AgentAttachmentReference {
        id: "a1".to_string(),
        conversation_id: "conversation".to_string(),
        message_id: "message".to_string(),
        project_id: None,
        kind: AgentInputAttachmentKind::File,
        name: "source.docx".to_string(),
        mime_type: Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string(),
        ),
        size_bytes: fs::metadata(&stored).unwrap().len(),
        read_path: read_path.clone(),
        storage_rel_path: "stored.docx".to_string(),
        created_at: 1,
    };
    let library = AgentAttachmentLibraryContext {
        root_path: Some(library_root.path().to_string_lossy().into_owned()),
        conversation_id: Some("conversation".to_string()),
        project_id: None,
        conversation_attachments: vec![reference],
        project_attachments: Vec::new(),
        folder_references: Vec::new(),
    };
    let permissions = AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        ..AgentPermissions::default()
    };
    let context = OfficeExecutionContext::new(
        Some(fixture.workspace.path().to_path_buf()),
        permissions,
        Some(library),
    );
    let request = OfficeExecutionRequest {
        document_path: Some(read_path.clone()),
        ..fixture.request(OfficeOperation::Validate)
    };
    let prepared = fixture.engine.prepare(&context, &request).unwrap();
    assert_eq!(
        prepared_path(&prepared, &OfficePathSlot::Document).scope,
        OfficePathScope::Attachment
    );

    let mutation = OfficeExecutionRequest {
        document_path: Some(read_path),
        ..fixture.request(OfficeOperation::Set)
    };
    let error = fixture.engine.prepare(&context, &mutation).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
}

#[test]
fn execution_rechecks_current_permissions() {
    let fixture = Fixture::new(basic_script());
    let external = tempfile::tempdir().unwrap();
    let request = OfficeExecutionRequest {
        document_kind: OfficeDocumentKind::Document,
        operation: OfficeOperation::Create,
        document_path: Some(
            external
                .path()
                .canonicalize()
                .unwrap()
                .join("created.docx")
                .to_string_lossy()
                .into_owned(),
        ),
        parameters: OfficeOperationParameters::Create {
            locale: None,
            minimal: false,
            overwrite: false,
        },
        output_path: None,
        destination_path: None,
        inputs: Vec::new(),
        timeout_ms: Some(10_000),
    };
    let all = permission_context(None, AgentReadPermission::All, AgentWritePermission::All);
    let prepared = fixture.engine.prepare(&all, &request).unwrap();
    let restricted = permission_context(
        Some(fixture.workspace.path()),
        AgentReadPermission::All,
        AgentWritePermission::WorkspaceOnly,
    );
    let error = fixture
        .engine
        .execute_prepared(&restricted, &prepared, AgentCancellationToken::new(), None)
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
}

#[test]
fn system_alias_targets_are_supported_only_with_write_all() {
    let fixture = Fixture::new(basic_script());
    let path = format!("@home/.mycopilot-office-{}.docx", uuid::Uuid::new_v4());
    let request = OfficeExecutionRequest {
        document_kind: OfficeDocumentKind::Document,
        operation: OfficeOperation::Create,
        document_path: Some(path),
        parameters: OfficeOperationParameters::Create {
            locale: None,
            minimal: false,
            overwrite: false,
        },
        output_path: None,
        destination_path: None,
        inputs: Vec::new(),
        timeout_ms: Some(10_000),
    };
    let error = fixture
        .engine
        .prepare(
            &permission_context(
                Some(fixture.workspace.path()),
                AgentReadPermission::All,
                AgentWritePermission::WorkspaceOnly,
            ),
            &request,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
    let prepared = fixture
        .engine
        .prepare(
            &permission_context(None, AgentReadPermission::All, AgentWritePermission::All),
            &request,
        )
        .unwrap();
    assert_eq!(
        prepared_path(&prepared, &OfficePathSlot::Document).scope,
        OfficePathScope::External
    );
}

fn multi_workspace_office_context(primary: &Path, auxiliary: &Path) -> OfficeExecutionContext {
    use crate::storage::models::{ProjectFolderRecord, ProjectFolderRole, ProjectRecord};
    let mut project =
        ProjectRecord::with_primary_folder("project", "Project", primary.to_string_lossy(), 0);
    project.folders[0].alias = "app".into();
    project.folders.push(ProjectFolderRecord {
        id: "folder-docs".into(),
        alias: "docs".into(),
        path: auxiliary.to_string_lossy().into_owned(),
        role: ProjectFolderRole::Auxiliary,
        sort_order: 1,
        created_at: 0,
    });
    let frozen = crate::workspace::freeze_project_workspace(&project).unwrap();
    OfficeExecutionContext::new(
        Some(primary.to_path_buf()),
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            ..AgentPermissions::default()
        },
        None,
    )
    .with_workspace(Some(&frozen))
}

#[test]
fn multi_workspace_office_reads_and_commits_auxiliary_destinations_with_workspace_permissions() {
    let replacement = tempfile::NamedTempFile::new().unwrap();
    write_docx(replacement.path(), "replacement");
    let fixture = Fixture::new(&format!(
        "#!/bin/sh\n/bin/cp '{}' \"$2\"\n",
        replacement.path().display()
    ));
    let auxiliary = tempfile::tempdir().unwrap();
    write_docx(&fixture.workspace.path().join("sample.docx"), "primary");
    write_docx(&auxiliary.path().join("sample.docx"), "auxiliary");
    let context = multi_workspace_office_context(fixture.workspace.path(), auxiliary.path());
    let mut request = fixture.request(OfficeOperation::Set);
    request.document_path = Some("@workspace/docs/sample.docx".into());
    request.destination_path = Some("@workspace/docs/copy.docx".into());
    let prepared = fixture.engine.prepare(&context, &request).unwrap();
    assert_eq!(
        prepared_path(&prepared, &OfficePathSlot::Document).scope,
        OfficePathScope::Workspace
    );
    assert_eq!(
        prepared_path(&prepared, &OfficePathSlot::Destination).scope,
        OfficePathScope::Workspace
    );
    let result = fixture
        .engine
        .execute_prepared(&context, &prepared, AgentCancellationToken::new(), None)
        .unwrap();
    assert!(result.error_code.is_none(), "{:?}", result.error);
    assert_eq!(
        fs::read(auxiliary.path().join("copy.docx")).unwrap(),
        fs::read(replacement.path()).unwrap()
    );
    assert!(!fixture.workspace.path().join("copy.docx").exists());
    request.destination_path = Some("@workspace/unknown/copy.docx".into());
    assert!(fixture.engine.prepare(&context, &request).is_err());
}

#[test]
fn multi_workspace_office_approval_rejects_replaced_auxiliary_directory() {
    let fixture = Fixture::new("#!/bin/sh\nexit 0\n");
    let auxiliary = tempfile::tempdir().unwrap();
    write_docx(&auxiliary.path().join("sample.docx"), "source");
    let context = multi_workspace_office_context(fixture.workspace.path(), auxiliary.path());
    let mut request = fixture.request(OfficeOperation::Set);
    request.document_path = Some("@workspace/docs/sample.docx".into());
    let prepared = fixture.engine.prepare(&context, &request).unwrap();
    let old = auxiliary.path().with_extension("old");
    fs::rename(auxiliary.path(), &old).unwrap();
    fs::create_dir(auxiliary.path()).unwrap();
    fs::copy(
        old.join("sample.docx"),
        auxiliary.path().join("sample.docx"),
    )
    .unwrap();
    assert!(fixture
        .engine
        .execute_prepared(&context, &prepared, AgentCancellationToken::new(), None)
        .is_err());
    fs::remove_dir_all(old).unwrap();
}

#[test]
fn multi_workspace_office_trusted_engine_cannot_be_loaded_from_auxiliary_folder() {
    let fixture = Fixture::new("#!/bin/sh\nexit 0\n");
    write_docx(&fixture.workspace.path().join("sample.docx"), "source");
    let context =
        multi_workspace_office_context(fixture.workspace.path(), fixture.engine_dir.path());
    let error = fixture
        .engine
        .prepare(&context, &fixture.request(OfficeOperation::Set))
        .unwrap_err();
    assert!(error
        .message()
        .contains("every agent-writable workspace folder"));

    let runtime = fixture.engine.render_runtime().unwrap();
    let component_child = runtime.executable_path().parent().unwrap();
    let nested_context = multi_workspace_office_context(fixture.workspace.path(), component_child);
    assert!(fixture
        .engine
        .prepare(&nested_context, &fixture.request(OfficeOperation::Set))
        .is_err());
}
