use super::*;
use std::io::Write;
use tempfile::tempdir;
use zip::write::SimpleFileOptions;

fn permissions_with(read: AgentReadPermission, write: AgentWritePermission) -> AgentPermissions {
    AgentPermissions {
        read,
        write,
        ..AgentPermissions::default()
    }
}

fn permissions(write: AgentWritePermission) -> AgentPermissions {
    permissions_with(AgentReadPermission::WorkspaceOnly, write)
}

fn observe(expected_outputs: &[&str]) -> AgentCommandArtifactObservationRequest {
    AgentCommandArtifactObservationRequest {
        kinds: vec![AgentCommandArtifactObservationKind::Office],
        expected_outputs: expected_outputs
            .iter()
            .map(|path| (*path).to_string())
            .collect(),
        additional_roots: Vec::new(),
    }
}

fn write_ooxml(path: &Path, main_part: &str, marker: &str) {
    let file = File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    archive.start_file("[Content_Types].xml", options).unwrap();
    archive
        .write_all(
            b"<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>",
        )
        .unwrap();
    archive.start_file(main_part, options).unwrap();
    archive
        .write_all(format!("<main>{marker}</main>").as_bytes())
        .unwrap();
    archive.finish().unwrap();
}

fn write_workbook(path: &Path, marker: &str) {
    write_ooxml(path, "xl/workbook.xml", marker);
}

fn append_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn append_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn append_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn append_classic_eocd(
    bytes: &mut Vec<u8>,
    entry_count: u16,
    directory_size: u32,
    directory_offset: u32,
    comment_len: u16,
) {
    bytes.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
    append_u16(bytes, 0);
    append_u16(bytes, 0);
    append_u16(bytes, entry_count);
    append_u16(bytes, entry_count);
    append_u32(bytes, directory_size);
    append_u32(bytes, directory_offset);
    append_u16(bytes, comment_len);
}

fn zip64_metadata_stub(
    entry_count: u64,
    directory_size: u64,
    directory_offset: u64,
    zip64_payload_size: u64,
    locator_record_offset: u64,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0x50, 0x4b, 0x06, 0x06]);
    append_u64(&mut bytes, zip64_payload_size);
    append_u16(&mut bytes, 45);
    append_u16(&mut bytes, 45);
    append_u32(&mut bytes, 0);
    append_u32(&mut bytes, 0);
    append_u64(&mut bytes, entry_count);
    append_u64(&mut bytes, entry_count);
    append_u64(&mut bytes, directory_size);
    append_u64(&mut bytes, directory_offset);
    debug_assert_eq!(bytes.len(), ZIP64_EOCD_FIXED_BYTES);

    bytes.extend_from_slice(&[0x50, 0x4b, 0x06, 0x07]);
    append_u32(&mut bytes, 0);
    append_u64(&mut bytes, locator_record_offset);
    append_u32(&mut bytes, 1);
    append_classic_eocd(&mut bytes, u16::MAX, u32::MAX, u32::MAX, 0);
    bytes
}

fn rewrite_with_zip64_eocd(mut bytes: Vec<u8>) -> Vec<u8> {
    let classic_offset = find_eocd_at_physical_end(&bytes).unwrap();
    assert_eq!(classic_offset + ZIP_EOCD_FIXED_BYTES, bytes.len());
    let classic = bytes[classic_offset..].to_vec();
    let entry_count = read_u16_le(&classic, 10) as u64;
    let directory_size = read_u32_le(&classic, 12) as u64;
    let directory_offset = read_u32_le(&classic, 16) as u64;
    bytes.truncate(classic_offset);
    let zip64_eocd_offset = bytes.len() as u64;

    bytes.extend_from_slice(&[0x50, 0x4b, 0x06, 0x06]);
    append_u64(&mut bytes, 44);
    append_u16(&mut bytes, 45);
    append_u16(&mut bytes, 45);
    append_u32(&mut bytes, 0);
    append_u32(&mut bytes, 0);
    append_u64(&mut bytes, entry_count);
    append_u64(&mut bytes, entry_count);
    append_u64(&mut bytes, directory_size);
    append_u64(&mut bytes, directory_offset);
    bytes.extend_from_slice(&[0x50, 0x4b, 0x06, 0x07]);
    append_u32(&mut bytes, 0);
    append_u64(&mut bytes, zip64_eocd_offset);
    append_u32(&mut bytes, 1);
    append_classic_eocd(&mut bytes, u16::MAX, u32::MAX, u32::MAX, 0);
    bytes
}

#[test]
fn bounded_zip_preflight_preserves_normal_ooxml_kinds() {
    let fixture = tempdir().unwrap();
    let cases = [
        (
            "document.docx",
            "word/document.xml",
            AgentCommandArtifactKind::Document,
        ),
        (
            "workbook.xlsx",
            "xl/workbook.xml",
            AgentCommandArtifactKind::Spreadsheet,
        ),
        (
            "slides.pptx",
            "ppt/presentation.xml",
            AgentCommandArtifactKind::Presentation,
        ),
    ];

    for (name, main_part, kind) in cases {
        let path = fixture.path().join(name);
        write_ooxml(&path, main_part, name);
        let mut preflight_file = File::open(&path).unwrap();
        let layout = preflight_ooxml_zip(&mut preflight_file).unwrap();
        assert_eq!(layout.entry_count, 2);
        let validation = validate_office_artifact(&path, kind, File::open(&path).unwrap());
        assert_eq!(
            validation.status,
            AgentCommandArtifactValidationStatus::Valid
        );
    }
}

#[test]
fn bounded_zip_preflight_accepts_valid_small_zip64_ooxml() {
    let fixture = tempdir().unwrap();
    let path = fixture.path().join("zip64.xlsx");
    write_workbook(&path, "zip64");
    let classic = fs::read(&path).unwrap();
    fs::write(&path, rewrite_with_zip64_eocd(classic)).unwrap();

    let mut file = File::open(&path).unwrap();
    let layout = preflight_ooxml_zip(&mut file).unwrap();
    assert_eq!(layout.entry_count, 2);
    let validation = validate_office_artifact(
        &path,
        AgentCommandArtifactKind::Spreadsheet,
        File::open(&path).unwrap(),
    );
    assert_eq!(
        validation.status,
        AgentCommandArtifactValidationStatus::Valid
    );
}

#[test]
fn bounded_zip_preflight_rejects_huge_zip64_declarations_before_zip_parser() {
    let fixture = tempdir().unwrap();

    let too_many_path = fixture.path().join("too-many.xlsx");
    fs::write(
        &too_many_path,
        zip64_metadata_stub(MAX_OOXML_ENTRIES as u64 + 1, 0, 0, 44, 0),
    )
    .unwrap();
    let error = preflight_ooxml_zip(&mut File::open(&too_many_path).unwrap()).unwrap_err();
    assert_eq!(
        error.code,
        "command.artifact.validation.too_many_ooxml_entries"
    );

    let huge_directory_path = fixture.path().join("huge-directory.xlsx");
    fs::write(
        &huge_directory_path,
        zip64_metadata_stub(0, MAX_OOXML_CENTRAL_DIRECTORY_BYTES + 1, 0, 44, 0),
    )
    .unwrap();
    let error = preflight_ooxml_zip(&mut File::open(&huge_directory_path).unwrap()).unwrap_err();
    assert_eq!(
        error.code,
        "command.artifact.validation.ooxml_central_directory_too_large"
    );

    let huge_record_path = fixture.path().join("huge-record.xlsx");
    fs::write(
        &huge_record_path,
        zip64_metadata_stub(0, 0, 0, MAX_ZIP64_EOCD_RECORD_BYTES, 0),
    )
    .unwrap();
    let error = preflight_ooxml_zip(&mut File::open(&huge_record_path).unwrap()).unwrap_err();
    assert_eq!(
        error.code,
        "command.artifact.validation.zip64_eocd_too_large"
    );
}

#[test]
fn bounded_zip_preflight_rejects_truncated_and_inconsistent_layouts() {
    let fixture = tempdir().unwrap();

    let truncated_path = fixture.path().join("truncated.xlsx");
    let mut truncated = Vec::new();
    append_classic_eocd(&mut truncated, 0, 0, 0, 1);
    fs::write(&truncated_path, truncated).unwrap();
    assert!(preflight_ooxml_zip(&mut File::open(&truncated_path).unwrap()).is_err());

    let inconsistent_path = fixture.path().join("inconsistent.xlsx");
    let mut inconsistent = Vec::new();
    append_classic_eocd(&mut inconsistent, 1, 46, 0, 0);
    fs::write(&inconsistent_path, inconsistent).unwrap();
    let error = preflight_ooxml_zip(&mut File::open(&inconsistent_path).unwrap()).unwrap_err();
    assert_eq!(error.code, "command.artifact.validation.invalid_ooxml_zip");

    let bad_locator_path = fixture.path().join("bad-locator.xlsx");
    fs::write(
        &bad_locator_path,
        zip64_metadata_stub(0, 0, 0, 44, u64::MAX),
    )
    .unwrap();
    assert!(preflight_ooxml_zip(&mut File::open(&bad_locator_path).unwrap()).is_err());
}

#[test]
fn bounded_zip_preflight_handles_maximum_classic_comment_without_overread() {
    let fixture = tempdir().unwrap();
    let path = fixture.path().join("max-comment.xlsx");
    let mut bytes = Vec::with_capacity(ZIP_EOCD_MAX_SEARCH_BYTES);
    append_classic_eocd(&mut bytes, 0, 0, 0, u16::MAX);
    bytes.resize(ZIP_EOCD_MAX_SEARCH_BYTES, b'x');
    fs::write(&path, bytes).unwrap();

    let layout = preflight_ooxml_zip(&mut File::open(&path).unwrap()).unwrap();
    assert_eq!(layout.entry_count, 0);
    assert_eq!(layout.central_directory_size, 0);
    assert_eq!(layout.central_directory_offset, 0);
}

fn capture_with_coverage(
    files: BTreeMap<PathBuf, ObservedArtifact>,
    complete_roots: Vec<PathBuf>,
    excluded_roots: Vec<PathBuf>,
    truncated: bool,
) -> CommandArtifactCapture {
    CommandArtifactCapture {
        files,
        complete_roots,
        excluded_roots,
        coverage: AgentCommandArtifactSnapshotCoverage {
            truncated,
            ..AgentCommandArtifactSnapshotCoverage::default()
        },
        warnings: Vec::new(),
    }
}

#[cfg(unix)]
#[test]
fn reports_created_modified_replaced_deleted_and_unambiguous_rename() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().canonicalize().unwrap();
    write_workbook(&workspace.join("modified.xlsx"), "modified-before");
    write_workbook(&workspace.join("replaced.xlsx"), "replaced-before");
    write_workbook(&workspace.join("deleted.xlsx"), "deleted-before");
    write_workbook(&workspace.join("rename-old.xlsx"), "rename-marker");
    write_workbook(&workspace.join("unchanged.xlsx"), "unchanged-marker");

    let observer = CommandArtifactObserver::prepare(
        Some(&workspace),
        &workspace,
        Some(&observe(&[
            "created.xlsx",
            "unchanged.xlsx",
            "missing.xlsx",
            "invalid.xlsx",
            "rename-old.xlsx",
        ])),
        permissions(AgentWritePermission::WorkspaceOnly),
    )
    .unwrap();
    let before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);

    write_workbook(&workspace.join("created.xlsx"), "created-after");
    write_workbook(&workspace.join("modified.xlsx"), "modified-after");
    write_workbook(&workspace.join("replacement.tmp"), "replaced-after");
    fs::rename(
        workspace.join("replacement.tmp"),
        workspace.join("replaced.xlsx"),
    )
    .unwrap();
    fs::remove_file(workspace.join("deleted.xlsx")).unwrap();
    fs::rename(
        workspace.join("rename-old.xlsx"),
        workspace.join("rename-new.xlsx"),
    )
    .unwrap();
    fs::write(workspace.join("invalid.xlsx"), b"not an OOXML package").unwrap();

    let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
    let result = observer.finish(before, after);
    let changes = result
        .changes
        .iter()
        .map(|change| (change.path.as_str(), change.kind))
        .collect::<StdHashMap<_, _>>();

    assert_eq!(
        changes.get("created.xlsx"),
        Some(&AgentCommandArtifactChangeKind::Created)
    );
    assert_eq!(
        changes.get("modified.xlsx"),
        Some(&AgentCommandArtifactChangeKind::Modified)
    );
    assert_eq!(
        changes.get("replaced.xlsx"),
        Some(&AgentCommandArtifactChangeKind::Replaced)
    );
    assert_eq!(
        changes.get("deleted.xlsx"),
        Some(&AgentCommandArtifactChangeKind::Deleted)
    );
    assert_eq!(
        changes.get("rename-new.xlsx"),
        Some(&AgentCommandArtifactChangeKind::Renamed)
    );
    let renamed = result
        .changes
        .iter()
        .find(|change| change.kind == AgentCommandArtifactChangeKind::Renamed)
        .unwrap();
    assert_eq!(renamed.previous_path.as_deref(), Some("rename-old.xlsx"));

    let outcomes = result
        .expected_outputs
        .iter()
        .map(|outcome| (outcome.requested_path.as_str(), outcome.outcome))
        .collect::<StdHashMap<_, _>>();
    assert_eq!(
        outcomes.get("created.xlsx"),
        Some(&AgentCommandExpectedArtifactOutcomeKind::Created)
    );
    assert_eq!(
        outcomes.get("unchanged.xlsx"),
        Some(&AgentCommandExpectedArtifactOutcomeKind::Unchanged)
    );
    assert_eq!(
        outcomes.get("missing.xlsx"),
        Some(&AgentCommandExpectedArtifactOutcomeKind::Missing)
    );
    assert_eq!(
        outcomes.get("invalid.xlsx"),
        Some(&AgentCommandExpectedArtifactOutcomeKind::Invalid)
    );
    assert_eq!(
        outcomes.get("rename-old.xlsx"),
        Some(&AgentCommandExpectedArtifactOutcomeKind::Renamed)
    );
    assert!(result
        .warnings
        .iter()
        .any(|warning| { warning.code == "command.artifact.expected_output.missing" }));
    assert!(result
        .warnings
        .iter()
        .any(|warning| { warning.code == "command.artifact.expected_output.unchanged" }));
    assert!(result
        .warnings
        .iter()
        .any(|warning| { warning.code == "command.artifact.expected_output.invalid" }));
}

#[test]
fn external_hint_does_not_expand_workspace_only_write_scope() {
    let workspace_fixture = tempdir().unwrap();
    let outside_fixture = tempdir().unwrap();
    let workspace = workspace_fixture.path().canonicalize().unwrap();
    let outside = outside_fixture
        .path()
        .join("external.xlsx")
        .to_string_lossy()
        .to_string();
    let observer = CommandArtifactObserver::prepare(
        Some(&workspace),
        &workspace,
        Some(&observe(&[&outside])),
        permissions(AgentWritePermission::WorkspaceOnly),
    )
    .unwrap();
    let before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);
    let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
    let result = observer.finish(before, after);

    assert_eq!(
        result.expected_outputs[0].outcome,
        AgentCommandExpectedArtifactOutcomeKind::Unobserved
    );
    assert!(result
        .warnings
        .iter()
        .any(|warning| { warning.code == "command.artifact.path.outside_write_scope" }));
    assert_eq!(
        result.status,
        AgentCommandArtifactObservationStatus::Partial
    );
}

#[test]
fn external_additional_root_requires_read_all() {
    let workspace_fixture = tempdir().unwrap();
    let outside_fixture = tempdir().unwrap();
    let workspace = workspace_fixture.path().canonicalize().unwrap();
    let outside = outside_fixture.path().canonicalize().unwrap();
    let outside_file = outside.join("secret.xlsx");
    write_workbook(&outside_file, "outside");
    let request = AgentCommandArtifactObservationRequest {
        kinds: vec![AgentCommandArtifactObservationKind::Office],
        expected_outputs: Vec::new(),
        additional_roots: vec![outside.to_string_lossy().to_string()],
    };

    let restricted = CommandArtifactObserver::prepare(
        Some(&workspace),
        &workspace,
        Some(&request),
        permissions_with(
            AgentReadPermission::WorkspaceOnly,
            AgentWritePermission::All,
        ),
    )
    .unwrap();
    let before = restricted.capture(AgentCommandArtifactObservationPhase::Before, None);
    let after = restricted.capture(AgentCommandArtifactObservationPhase::After, None);
    let result = restricted.finish(before, after);
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.code == "command.artifact.path.outside_read_scope"));
    assert_eq!(result.coverage.additional_root_count, 0);

    let allowed = CommandArtifactObserver::prepare(
        Some(&workspace),
        &workspace,
        Some(&request),
        permissions_with(
            AgentReadPermission::All,
            AgentWritePermission::WorkspaceOnly,
        ),
    )
    .unwrap();
    let snapshot = allowed.capture(AgentCommandArtifactObservationPhase::Before, None);
    assert!(snapshot.files.contains_key(&outside_file));
}

#[test]
fn external_expected_output_observes_only_the_exact_target() {
    let workspace_fixture = tempdir().unwrap();
    let outside_fixture = tempdir().unwrap();
    let workspace = workspace_fixture.path().canonicalize().unwrap();
    let outside = outside_fixture.path().canonicalize().unwrap();
    let expected = outside.join("expected.xlsx");
    let sibling = outside.join("private-sibling.xlsx");
    write_workbook(&expected, "expected");
    write_workbook(&sibling, "sibling");
    let expected_text = expected.to_string_lossy().to_string();
    let observer = CommandArtifactObserver::prepare(
        Some(&workspace),
        &workspace,
        Some(&observe(&[&expected_text])),
        permissions_with(
            AgentReadPermission::WorkspaceOnly,
            AgentWritePermission::All,
        ),
    )
    .unwrap();

    let snapshot = observer.capture(AgentCommandArtifactObservationPhase::Before, None);
    assert!(snapshot.files.contains_key(&expected));
    assert!(!snapshot.files.contains_key(&sibling));
    assert_eq!(snapshot.coverage.office_files_seen, 1);
}

#[cfg(unix)]
#[test]
fn workspace_scan_never_follows_symlinks() {
    use std::os::unix::fs::symlink;

    let workspace_fixture = tempdir().unwrap();
    let outside_fixture = tempdir().unwrap();
    let workspace = workspace_fixture.path().canonicalize().unwrap();
    write_workbook(&outside_fixture.path().join("secret.xlsx"), "outside");
    symlink(outside_fixture.path(), workspace.join("linked-output")).unwrap();
    let observer = CommandArtifactObserver::prepare(
        Some(&workspace),
        &workspace,
        Some(&observe(&[])),
        permissions(AgentWritePermission::WorkspaceOnly),
    )
    .unwrap();
    let before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);

    assert!(before.files.is_empty());
    assert_eq!(before.coverage.symlinks_skipped, 1);
    assert!(before
        .warnings
        .iter()
        .any(|warning| warning.code == "command.artifact.symlink_skipped"));
}

#[test]
fn file_and_hash_budgets_produce_explicit_partial_coverage() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().canonicalize().unwrap();
    fs::write(workspace.join("one.csv"), b"one").unwrap();
    fs::write(workspace.join("two.csv"), b"two").unwrap();
    let mut observer = CommandArtifactObserver::prepare(
        Some(&workspace),
        &workspace,
        Some(&observe(&[])),
        permissions(AgentWritePermission::WorkspaceOnly),
    )
    .unwrap();
    observer.budget.max_office_files = 1;
    let before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);
    let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
    let result = observer.finish(before, after);

    assert!(result.coverage.before.truncated);
    assert_eq!(result.coverage.before.office_files_seen, 2);
    assert_eq!(
        result.status,
        AgentCommandArtifactObservationStatus::Partial
    );
    assert!(result.partial);
    assert_eq!(result.scanned, 4);
    assert_eq!(result.returned, result.changes.len() as u64);
    assert_eq!(result.omitted, result.changes_omitted);
    assert!(result
        .stop_reasons
        .iter()
        .any(|reason| reason == "before_scan_limit"));
    assert!(result
        .stop_reasons
        .iter()
        .any(|reason| reason == "after_scan_limit"));

    let mut hash_limited = CommandArtifactObserver::prepare(
        Some(&workspace),
        &workspace,
        Some(&observe(&[])),
        permissions(AgentWritePermission::WorkspaceOnly),
    )
    .unwrap();
    hash_limited.budget.max_hashed_bytes = 1;
    let snapshot = hash_limited.capture(AgentCommandArtifactObservationPhase::Before, None);
    assert!(snapshot.coverage.truncated);
    assert!(snapshot.coverage.files_unhashed >= 1);
    assert!(snapshot
        .warnings
        .iter()
        .any(|warning| warning.code == "command.artifact.hash.total_budget"));
}

#[test]
fn snapshots_report_time_budget_and_before_cancellation() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().canonicalize().unwrap();
    fs::write(workspace.join("artifact.csv"), b"value").unwrap();

    let mut timed = CommandArtifactObserver::prepare(
        Some(&workspace),
        &workspace,
        Some(&observe(&[])),
        permissions(AgentWritePermission::WorkspaceOnly),
    )
    .unwrap();
    timed.budget.max_duration = Duration::ZERO;
    let timed_snapshot = timed.capture(AgentCommandArtifactObservationPhase::Before, None);
    assert!(timed_snapshot.coverage.truncated);
    assert!(timed_snapshot.coverage.time_budget_exceeded);
    assert!(timed_snapshot
        .warnings
        .iter()
        .any(|warning| warning.code == "command.artifact.scan.time_budget"));

    let observer = CommandArtifactObserver::prepare(
        Some(&workspace),
        &workspace,
        Some(&observe(&[])),
        permissions(AgentWritePermission::WorkspaceOnly),
    )
    .unwrap();
    let cancellation = AgentCancellationToken::new();
    cancellation.cancel();
    let before = observer.capture(
        AgentCommandArtifactObservationPhase::Before,
        Some(&cancellation),
    );
    assert!(before.coverage.cancelled);
    assert!(before.coverage.truncated);
    assert!(before.files.is_empty());

    // An after snapshot deliberately does not inherit command cancellation: a cancelled
    // process may already have created or modified an artifact.
    let after = observer.capture(
        AgentCommandArtifactObservationPhase::After,
        Some(&cancellation),
    );
    assert!(!after.coverage.cancelled);
    assert!(after.files.contains_key(&workspace.join("artifact.csv")));
}

#[cfg(unix)]
#[test]
fn unstable_directory_identity_discards_subtree_results() {
    let fixture = tempdir().unwrap();
    let directory = fixture.path().join("observed");
    fs::create_dir(&directory).unwrap();
    let identity_before = stable_directory_identity(&directory).unwrap();
    let observed_file = directory.join("artifact.csv");
    let mut state = CaptureState {
        files: BTreeMap::from([(
            observed_file,
            ObservedArtifact {
                kind: AgentCommandArtifactKind::Spreadsheet,
                metadata: AgentCommandArtifactMetadata {
                    size_bytes: 1,
                    sha256: Some("digest".to_string()),
                    validation: AgentCommandArtifactValidation {
                        status: AgentCommandArtifactValidationStatus::NotApplicable,
                        code: None,
                        message: None,
                    },
                },
                identity: None,
                modified_ns: None,
            },
        )]),
        complete_roots: vec![directory.clone()],
        excluded_roots: Vec::new(),
        coverage: AgentCommandArtifactSnapshotCoverage::default(),
        warnings: Vec::new(),
        phase: AgentCommandArtifactObservationPhase::Before,
        budget: ObservationBudget::default(),
        deadline: Instant::now() + Duration::from_secs(1),
        cancellation_token: None,
    };
    let before = capture_with_coverage(
        state.files.clone(),
        state.complete_roots.clone(),
        Vec::new(),
        false,
    );
    fs::rename(&directory, fixture.path().join("original")).unwrap();
    fs::create_dir(&directory).unwrap();
    let mut root_complete = true;

    verify_directory_identity(&directory, identity_before, &mut state, &mut root_complete);

    assert!(!root_complete);
    assert!(state.files.is_empty());
    assert!(state.complete_roots.is_empty());
    assert!(state
        .warnings
        .iter()
        .any(|warning| { warning.code == "command.artifact.directory.identity_changed" }));
    let after = capture_with_coverage(
        state.files,
        state.complete_roots,
        state.excluded_roots,
        true,
    );
    assert!(diff_captures(&before, &after, None).is_empty());
}

#[test]
fn change_report_is_bounded_and_prioritizes_expected_outputs() {
    let expected_path = PathBuf::from("/outside/zz-expected.xlsx");
    let mut changes = (0..300)
        .map(|index| AgentCommandArtifactChange {
            kind: AgentCommandArtifactChangeKind::Created,
            artifact_kind: AgentCommandArtifactKind::Spreadsheet,
            path: format!("/outside/{index:03}.xlsx"),
            scope: AgentCommandArtifactScope::External,
            previous_path: None,
            previous_scope: None,
            before: None,
            after: None,
        })
        .collect::<Vec<_>>();
    changes.push(AgentCommandArtifactChange {
        kind: AgentCommandArtifactChangeKind::Created,
        artifact_kind: AgentCommandArtifactKind::Spreadsheet,
        path: display_path(&expected_path),
        scope: AgentCommandArtifactScope::External,
        previous_path: None,
        previous_scope: None,
        before: None,
        after: None,
    });
    let expected_outputs = vec![ExpectedOutput {
        requested_path: expected_path.to_string_lossy().to_string(),
        resolved_path: Some(expected_path.clone()),
    }];

    let (reported, omitted) = limit_reported_changes(changes, &expected_outputs, None);

    assert_eq!(reported.len(), MAX_REPORTED_CHANGES);
    assert_eq!(omitted, 45);
    assert!(reported
        .iter()
        .any(|change| change.path == display_path(&expected_path)));
    assert!(serde_json::to_vec(&reported).unwrap().len() <= MAX_REPORTED_CHANGE_BYTES);
}

#[test]
fn fixed_workspace_exclusions_are_reported_without_downgrading_coverage() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().canonicalize().unwrap();
    fs::create_dir(workspace.join(".git")).unwrap();
    fs::write(workspace.join(".git").join("ignored.xlsx"), b"not observed").unwrap();
    let observer = CommandArtifactObserver::prepare(
        Some(&workspace),
        &workspace,
        Some(&observe(&[])),
        permissions(AgentWritePermission::WorkspaceOnly),
    )
    .unwrap();
    let before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);
    let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
    let result = observer.finish(before, after);

    assert_eq!(result.coverage.before.excluded_directories, 1);
    assert_eq!(result.coverage.after.excluded_directories, 1);
    assert_eq!(
        result.status,
        AgentCommandArtifactObservationStatus::Complete
    );
    assert!(result.changes.is_empty());
    assert!(!path_is_covered(
        &workspace.join("node_modules").join("hidden.xlsx"),
        std::slice::from_ref(&workspace),
        &[workspace.join("node_modules")],
    ));
    assert!(path_is_covered(
        &workspace.join("node_modules").join("expected.xlsx"),
        &[
            workspace.clone(),
            workspace.join("node_modules").join("expected.xlsx"),
        ],
        &[workspace.join("node_modules")],
    ));
}

#[test]
fn partial_snapshot_absence_never_fabricates_create_delete_or_rename() {
    let artifact = |digest: &str| ObservedArtifact {
        kind: AgentCommandArtifactKind::Spreadsheet,
        metadata: AgentCommandArtifactMetadata {
            size_bytes: 10,
            sha256: Some(digest.to_string()),
            validation: AgentCommandArtifactValidation {
                status: AgentCommandArtifactValidationStatus::Valid,
                code: None,
                message: None,
            },
        },
        identity: None,
        modified_ns: None,
    };
    let before = capture_with_coverage(
        BTreeMap::from([
            (PathBuf::from("/tmp/deleted.xlsx"), artifact("deleted")),
            (PathBuf::from("/tmp/modified.xlsx"), artifact("before")),
            (PathBuf::from("/tmp/rename-old.xlsx"), artifact("rename")),
        ]),
        Vec::new(),
        Vec::new(),
        true,
    );
    let after = capture_with_coverage(
        BTreeMap::from([
            (PathBuf::from("/tmp/created.xlsx"), artifact("created")),
            (PathBuf::from("/tmp/modified.xlsx"), artifact("after")),
            (PathBuf::from("/tmp/rename-new.xlsx"), artifact("rename")),
        ]),
        Vec::new(),
        Vec::new(),
        true,
    );

    let changes = diff_captures(&before, &after, None);

    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "/tmp/modified.xlsx");
    assert_eq!(changes[0].kind, AgentCommandArtifactChangeKind::Modified);
}

#[test]
fn asymmetric_file_budget_does_not_turn_unscanned_files_into_changes() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().canonicalize().unwrap();
    fs::write(workspace.join("a.csv"), b"a").unwrap();
    fs::write(workspace.join("b.csv"), b"b").unwrap();
    let mut observer = CommandArtifactObserver::prepare(
        Some(&workspace),
        &workspace,
        Some(&observe(&[])),
        permissions(AgentWritePermission::WorkspaceOnly),
    )
    .unwrap();

    observer.budget.max_office_files = 1;
    let partial_before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);
    observer.budget.max_office_files = 10;
    let complete_after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
    let result = observer.finish(partial_before, complete_after);
    assert_eq!(
        result.status,
        AgentCommandArtifactObservationStatus::Partial
    );
    assert!(result.changes.is_empty());

    let mut observer = CommandArtifactObserver::prepare(
        Some(&workspace),
        &workspace,
        Some(&observe(&[])),
        permissions(AgentWritePermission::WorkspaceOnly),
    )
    .unwrap();
    observer.budget.max_office_files = 10;
    let complete_before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);
    observer.budget.max_office_files = 1;
    let partial_after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
    let result = observer.finish(complete_before, partial_after);
    assert_eq!(
        result.status,
        AgentCommandArtifactObservationStatus::Partial
    );
    assert!(result.changes.is_empty());
}

#[test]
fn expected_output_seen_only_after_an_uncovered_before_snapshot_is_unobserved() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().canonicalize().unwrap();
    let mut observer = CommandArtifactObserver::prepare(
        Some(&workspace),
        &workspace,
        Some(&observe(&["created-during-gap.xlsx"])),
        permissions(AgentWritePermission::WorkspaceOnly),
    )
    .unwrap();

    observer.budget.max_duration = Duration::ZERO;
    let before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);
    assert!(before.coverage.truncated);
    assert!(!path_is_covered(
        &workspace.join("created-during-gap.xlsx"),
        &before.complete_roots,
        &before.excluded_roots,
    ));

    write_workbook(&workspace.join("created-during-gap.xlsx"), "after");
    observer.budget.max_duration = Duration::from_secs(1);
    let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
    let result = observer.finish(before, after);

    assert!(result.changes.is_empty());
    assert_eq!(
        result.expected_outputs[0].outcome,
        AgentCommandExpectedArtifactOutcomeKind::Unobserved
    );
    assert!(result.warnings.iter().any(|warning| {
        warning.code == "command.artifact.expected_output.unobserved"
            && warning.path.as_deref() == Some("created-during-gap.xlsx")
    }));
}

#[test]
fn ambiguous_equal_digests_are_not_misreported_as_renames() {
    let metadata = AgentCommandArtifactMetadata {
        size_bytes: 10,
        sha256: Some("same".to_string()),
        validation: AgentCommandArtifactValidation {
            status: AgentCommandArtifactValidationStatus::Valid,
            code: None,
            message: None,
        },
    };
    let artifact = ObservedArtifact {
        kind: AgentCommandArtifactKind::Spreadsheet,
        metadata,
        identity: None,
        modified_ns: None,
    };
    let before = capture_with_coverage(
        BTreeMap::from([
            (PathBuf::from("/tmp/a.xlsx"), artifact.clone()),
            (PathBuf::from("/tmp/b.xlsx"), artifact.clone()),
        ]),
        vec![PathBuf::from("/tmp")],
        Vec::new(),
        false,
    );
    let after = capture_with_coverage(
        BTreeMap::from([
            (PathBuf::from("/tmp/c.xlsx"), artifact.clone()),
            (PathBuf::from("/tmp/d.xlsx"), artifact),
        ]),
        vec![PathBuf::from("/tmp")],
        Vec::new(),
        false,
    );
    let changes = diff_captures(&before, &after, None);

    assert_eq!(
        changes
            .iter()
            .filter(|change| change.kind == AgentCommandArtifactChangeKind::Deleted)
            .count(),
        2
    );
    assert_eq!(
        changes
            .iter()
            .filter(|change| change.kind == AgentCommandArtifactChangeKind::Created)
            .count(),
        2
    );
    assert!(!changes
        .iter()
        .any(|change| change.kind == AgentCommandArtifactChangeKind::Renamed));
}
