use super::*;
use crate::skills::digest::package_file_digest;
use crate::skills::model::{
    SkillId, SkillResourceIndex, SkillRevision, SkillSourceId, SkillSourceKind, SkillTrust,
};
use crate::skills::resource_runtime::{
    SkillResourceReader, SkillResourceReaderRef, SkillResourceSessionBinding,
    SkillResourceSourceError,
};
use std::collections::BTreeMap;
#[cfg(unix)]
use std::ffi::CString;
use std::fs;
#[cfg(unix)]
use std::io::Write as _;
#[cfg(unix)]
use std::os::fd::FromRawFd;
use std::sync::{Arc, Barrier, Mutex};
use tempfile::tempdir;

struct MemoryReader {
    bytes: BTreeMap<String, Vec<u8>>,
}

impl SkillResourceReader for MemoryReader {
    fn read(
        &self,
        expected: &SkillResourceDescriptor,
    ) -> Result<Vec<u8>, SkillResourceSourceError> {
        self.bytes
            .get(expected.path())
            .cloned()
            .ok_or_else(|| SkillResourceSourceError::Unavailable("fixture missing".to_string()))
    }
}

fn test_session(
    path: &str,
    kind: SkillResourceKind,
    descriptor_bytes: &[u8],
    reader_bytes: &[u8],
) -> (SkillResourceSession, SkillResourceUri) {
    let source_id = SkillSourceId::parse("installed:user").unwrap();
    let skill_id = SkillId::parse("installed:user:01234567-89ab-4def-8123-456789abcdef").unwrap();
    let revision =
        SkillRevision::parse(format!("skill-package-sha256-v3:{}", "a".repeat(64))).unwrap();
    let descriptor = SkillResourceDescriptor::new(
        path.to_string(),
        kind,
        descriptor_bytes.len() as u64,
        package_file_digest(descriptor_bytes),
    );
    let reader: SkillResourceReaderRef = Arc::new(MemoryReader {
        bytes: BTreeMap::from([(path.to_string(), reader_bytes.to_vec())]),
    });
    let resource_session = SkillResourceSession::from_bindings([SkillResourceSessionBinding {
        skill_id: skill_id.clone(),
        revision: revision.clone(),
        source_id,
        source_kind: SkillSourceKind::Installed,
        trust: SkillTrust::Untrusted,
        resources: SkillResourceIndex::new(vec![descriptor]),
        reader: Some(reader),
    }])
    .unwrap();
    let uri = resource_session.package_uris()[0]
        .resource(super::super::resource_runtime::SkillResourcePath::parse(path).unwrap());
    (resource_session, uri)
}

fn test_tree_session(
    entries: &[(&str, SkillResourceKind, &[u8], &[u8])],
) -> (SkillResourceSession, SkillPackageUri) {
    let source_id = SkillSourceId::parse("installed:user").unwrap();
    let skill_id = SkillId::parse("installed:user:11234567-89ab-4def-8123-456789abcdef").unwrap();
    let revision =
        SkillRevision::parse(format!("skill-package-sha256-v3:{}", "b".repeat(64))).unwrap();
    let descriptors = entries
        .iter()
        .map(|(path, kind, descriptor_bytes, _)| {
            SkillResourceDescriptor::new(
                (*path).to_string(),
                *kind,
                descriptor_bytes.len() as u64,
                package_file_digest(descriptor_bytes),
            )
        })
        .collect::<Vec<_>>();
    let reader: SkillResourceReaderRef = Arc::new(MemoryReader {
        bytes: entries
            .iter()
            .map(|(path, _, _, reader_bytes)| ((*path).to_string(), (*reader_bytes).to_vec()))
            .collect(),
    });
    let session = SkillResourceSession::from_bindings([SkillResourceSessionBinding {
        skill_id,
        revision,
        source_id,
        source_kind: SkillSourceKind::Installed,
        trust: SkillTrust::Untrusted,
        resources: SkillResourceIndex::new(descriptors),
        reader: Some(reader),
    }])
    .unwrap();
    let package = session.package_uris()[0].clone();
    (session, package)
}

fn test_request(
    uri: SkillResourceUri,
    root: &Path,
    destination: &str,
) -> SkillMaterializationRequest {
    SkillMaterializationRequest::new(
        uri,
        root,
        SkillMaterializationDestination::parse(destination).unwrap(),
    )
    .unwrap()
}

fn test_tree_request(
    package: SkillPackageUri,
    prefix: &str,
    root: &Path,
    destination: &str,
) -> SkillTemplateTreeMaterializationRequest {
    SkillTemplateTreeMaterializationRequest::new(
        package,
        SkillResourcePath::parse(prefix).unwrap(),
        root,
        SkillMaterializationDestination::parse(destination).unwrap(),
    )
    .unwrap()
}

fn staging_entries(root: &Path) -> Vec<String> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(STAGING_NAME_PREFIX))
        .collect()
}

#[test]
fn creates_one_asset_atomically_and_same_content_is_idempotent() {
    let workspace = tempdir().unwrap();
    let bytes = b"asset bytes\n";
    let (session, uri) = test_session("assets/report.txt", SkillResourceKind::Asset, bytes, bytes);
    let request = test_request(uri, workspace.path(), "outputs/report.txt");
    fs::create_dir(workspace.path().join("outputs")).unwrap();
    let materializer = SkillResourceMaterializer::new();

    let created = materializer.materialize(&session, &request).unwrap();
    assert_eq!(created.status(), SkillMaterializationStatus::Created);
    assert_eq!(created.destination().as_str(), "outputs/report.txt");
    assert_eq!(created.byte_length(), bytes.len() as u64);
    assert_eq!(created.bytes_written(), bytes.len() as u64);
    assert_eq!(
        fs::read(workspace.path().join("outputs/report.txt")).unwrap(),
        bytes
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(workspace.path().join("outputs/report.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(staging_entries(&workspace.path().join("outputs")).is_empty());

    let repeated = materializer.materialize(&session, &request).unwrap();
    assert_eq!(
        repeated.status(),
        SkillMaterializationStatus::AlreadyPresent
    );
    assert_eq!(repeated.bytes_written(), 0);
    assert_eq!(
        fs::read(workspace.path().join("outputs/report.txt")).unwrap(),
        bytes
    );
}

#[test]
fn templates_other_is_allowed_but_reference_and_script_are_denied() {
    let workspace = tempdir().unwrap();
    let (template_session, template_uri) = test_session(
        "templates/budget.xlsx",
        SkillResourceKind::Other,
        b"template",
        b"template",
    );
    let template_request = test_request(template_uri, workspace.path(), "budget.xlsx");
    assert_eq!(
        SkillResourceMaterializer::new()
            .materialize(&template_session, &template_request)
            .unwrap()
            .status(),
        SkillMaterializationStatus::Created
    );

    for (path, kind) in [
        ("references/guide.md", SkillResourceKind::Reference),
        ("scripts/run.sh", SkillResourceKind::Script),
    ] {
        let (denied_session, denied_uri) = test_session(path, kind, b"denied", b"denied");
        let denied_request = test_request(denied_uri, workspace.path(), "denied.txt");
        let error = SkillResourceMaterializer::new()
            .materialize(&denied_session, &denied_request)
            .unwrap_err();
        assert_eq!(
            error.code(),
            SkillMaterializationErrorCode::SourceKindDenied
        );
        assert!(!workspace.path().join("denied.txt").exists());
    }
}

#[test]
fn existing_different_content_and_non_file_targets_are_typed_conflicts() {
    let workspace = tempdir().unwrap();
    let (session, uri) = test_session(
        "assets/report.txt",
        SkillResourceKind::Asset,
        b"new",
        b"new",
    );
    fs::write(workspace.path().join("report.txt"), b"old").unwrap();
    let request = test_request(uri.clone(), workspace.path(), "report.txt");
    let error = SkillResourceMaterializer::new()
        .materialize(&session, &request)
        .unwrap_err();
    assert_eq!(error.code(), SkillMaterializationErrorCode::Conflict);
    assert_eq!(
        fs::read(workspace.path().join("report.txt")).unwrap(),
        b"old"
    );

    fs::create_dir(workspace.path().join("directory")).unwrap();
    let directory_request = test_request(uri, workspace.path(), "directory");
    assert_eq!(
        SkillResourceMaterializer::new()
            .materialize(&session, &directory_request)
            .unwrap_err()
            .code(),
        SkillMaterializationErrorCode::Conflict
    );
}

#[test]
fn destination_validation_rejects_traversal_absolute_and_reserved_trees() {
    for destination in [
        "../outside.txt",
        "/tmp/outside.txt",
        ".git/hooks/post-commit",
        "output/.git/config",
        ".HG/store/file",
        ".agents/skills/evil/SKILL.md",
        ".mycopilot-skill-stage-forged",
    ] {
        let error = SkillMaterializationDestination::parse(destination).unwrap_err();
        assert!(matches!(
            error.code(),
            SkillMaterializationErrorCode::InvalidDestination
                | SkillMaterializationErrorCode::ReservedDestination
        ));
    }
}

#[cfg(unix)]
#[test]
fn symlinked_parent_and_destination_never_touch_outside_files() {
    use std::os::unix::fs::symlink;

    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let (session, uri) = test_session(
        "assets/report.txt",
        SkillResourceKind::Asset,
        b"safe",
        b"safe",
    );
    let linked_root = workspace.path().join("linked-root");
    symlink(outside.path(), &linked_root).unwrap();
    let root_request = test_request(uri.clone(), &linked_root, "root-out.txt");
    let root_error = SkillResourceMaterializer::new()
        .materialize(&session, &root_request)
        .unwrap_err();
    assert_eq!(
        root_error.code(),
        SkillMaterializationErrorCode::InvalidWorkspace
    );
    assert!(!outside.path().join("root-out.txt").exists());

    symlink(outside.path(), workspace.path().join("linked")).unwrap();
    let parent_request = test_request(uri.clone(), workspace.path(), "linked/out.txt");
    let parent_error = SkillResourceMaterializer::new()
        .materialize(&session, &parent_request)
        .unwrap_err();
    assert_eq!(
        parent_error.code(),
        SkillMaterializationErrorCode::UnsafeParent
    );
    assert!(!outside.path().join("out.txt").exists());

    let canary = outside.path().join("canary.txt");
    fs::write(&canary, b"CANARY").unwrap();
    symlink(&canary, workspace.path().join("target.txt")).unwrap();
    let target_request = test_request(uri, workspace.path(), "target.txt");
    let target_error = SkillResourceMaterializer::new()
        .materialize(&session, &target_request)
        .unwrap_err();
    assert_eq!(
        target_error.code(),
        SkillMaterializationErrorCode::UnsafeDestination
    );
    assert_eq!(fs::read(canary).unwrap(), b"CANARY");
}

#[cfg(unix)]
#[test]
fn replaced_file_staging_inode_is_rejected_and_never_published_or_deleted() {
    use super::unix::{install_materialization_test_hook, MaterializationTestHookPoint};

    let workspace = tempdir().unwrap();
    let bytes = b"verified bytes";
    let (session, uri) = test_session("assets/report.txt", SkillResourceKind::Asset, bytes, bytes);
    let request = test_request(uri, workspace.path(), "report.txt");
    let replacement_name = Arc::new(Mutex::new(None::<String>));
    let captured_name = Arc::clone(&replacement_name);
    let _hook = install_materialization_test_hook(move |point, parent_fd, staging_name| {
        if point != MaterializationTestHookPoint::FileBeforePublish {
            return;
        }
        *captured_name.lock().unwrap() = Some(staging_name.to_string_lossy().into_owned());
        let moved = CString::new(".test-original-staging").unwrap();
        // SAFETY: all fds and C strings are live for these test-only calls.
        assert_eq!(
            unsafe { libc::renameat(parent_fd, staging_name.as_ptr(), parent_fd, moved.as_ptr()) },
            0
        );
        let fd = unsafe {
            libc::openat(
                parent_fd,
                staging_name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        assert!(fd >= 0);
        // SAFETY: fd is newly owned by this test.
        let mut replacement = unsafe { fs::File::from_raw_fd(fd) };
        replacement.write_all(b"attacker replacement").unwrap();
        replacement.sync_all().unwrap();
    });

    let error = SkillResourceMaterializer::new()
        .materialize(&session, &request)
        .unwrap_err();
    assert_eq!(
        error.code(),
        SkillMaterializationErrorCode::UnsafeDestination
    );
    assert!(!workspace.path().join("report.txt").exists());
    let replacement_name = replacement_name.lock().unwrap().clone().unwrap();
    assert_eq!(
        fs::read(workspace.path().join(replacement_name)).unwrap(),
        b"attacker replacement"
    );
}

#[cfg(unix)]
#[test]
fn workspace_root_rebind_is_detected_before_publication() {
    use super::unix::{install_materialization_test_hook, MaterializationTestHookPoint};

    let container = tempdir().unwrap();
    let workspace = container.path().join("workspace");
    let moved_workspace = container.path().join("workspace-moved");
    fs::create_dir(&workspace).unwrap();
    let bytes = b"verified bytes";
    let (session, uri) = test_session("assets/report.txt", SkillResourceKind::Asset, bytes, bytes);
    let request = test_request(uri, &workspace, "report.txt");
    let hook_workspace = workspace.clone();
    let hook_moved = moved_workspace.clone();
    let _hook = install_materialization_test_hook(move |point, _, _| {
        if point == MaterializationTestHookPoint::FileBeforePublish {
            fs::rename(&hook_workspace, &hook_moved).unwrap();
            fs::create_dir(&hook_workspace).unwrap();
        }
    });

    let error = SkillResourceMaterializer::new()
        .materialize(&session, &request)
        .unwrap_err();
    assert_eq!(error.code(), SkillMaterializationErrorCode::UnsafeParent);
    assert!(!workspace.join("report.txt").exists());
    assert!(!moved_workspace.join("report.txt").exists());
    assert!(staging_entries(&moved_workspace).is_empty());
}

#[cfg(unix)]
#[test]
fn multi_workspace_materialization_rejects_root_replaced_after_authorization_before_open() {
    let container = tempdir().unwrap();
    let workspace = container.path().join("auxiliary");
    fs::create_dir(&workspace).unwrap();
    let identity = crate::file_change::FileChangeDirectoryIdentity::read(&workspace).unwrap();
    let bytes = b"verified bytes";
    let (session, uri) = test_session("assets/report.txt", SkillResourceKind::Asset, bytes, bytes);
    let request =
        test_request(uri, &workspace, "report.txt").with_workspace_identity(identity.clone());
    let (tree_session, package) =
        test_tree_session(&[("templates/main.txt", SkillResourceKind::Other, bytes, bytes)]);
    let tree_request = test_tree_request(package, "templates", &workspace, "tree")
        .with_workspace_identity(identity);
    fs::rename(&workspace, container.path().join("old-auxiliary")).unwrap();
    fs::create_dir(&workspace).unwrap();
    let materializer = SkillResourceMaterializer::new();
    assert_eq!(
        materializer
            .materialize(&session, &request)
            .unwrap_err()
            .code(),
        SkillMaterializationErrorCode::InvalidWorkspace
    );
    assert_eq!(
        materializer
            .materialize_template_tree(&tree_session, &tree_request)
            .unwrap_err()
            .code(),
        SkillMaterializationErrorCode::InvalidWorkspace
    );
    assert_eq!(fs::read_dir(&workspace).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn replaced_tree_staging_inode_is_rejected_and_never_published_or_deleted() {
    use super::unix::{install_materialization_test_hook, MaterializationTestHookPoint};

    let workspace = tempdir().unwrap();
    let bytes = b"verified template";
    let (session, package) = test_tree_session(&[(
        "templates/kit/template.txt",
        SkillResourceKind::Other,
        bytes,
        bytes,
    )]);
    let request = test_tree_request(package, "templates/kit", workspace.path(), "kit");
    let replacement_name = Arc::new(Mutex::new(None::<String>));
    let captured_name = Arc::clone(&replacement_name);
    let _hook = install_materialization_test_hook(move |point, parent_fd, staging_name| {
        if point != MaterializationTestHookPoint::TreeBeforePublish {
            return;
        }
        *captured_name.lock().unwrap() = Some(staging_name.to_string_lossy().into_owned());
        let moved = CString::new(".test-original-staging-tree").unwrap();
        // SAFETY: all fds and C strings are live for these test-only calls.
        assert_eq!(
            unsafe { libc::renameat(parent_fd, staging_name.as_ptr(), parent_fd, moved.as_ptr()) },
            0
        );
        assert_eq!(
            unsafe { libc::mkdirat(parent_fd, staging_name.as_ptr(), 0o700) },
            0
        );
    });

    let error = SkillResourceMaterializer::new()
        .materialize_template_tree(&session, &request)
        .unwrap_err();
    assert_eq!(
        error.code(),
        SkillMaterializationErrorCode::UnsafeDestination
    );
    assert!(!workspace.path().join("kit").exists());
    let replacement_name = replacement_name.lock().unwrap().clone().unwrap();
    assert!(workspace.path().join(replacement_name).is_dir());
    assert_eq!(
        fs::read(
            workspace
                .path()
                .join(".test-original-staging-tree/template.txt")
        )
        .unwrap(),
        bytes
    );
}

#[test]
fn missing_parent_and_tampered_source_fail_without_partial_files() {
    let workspace = tempdir().unwrap();
    let (session, uri) = test_session(
        "assets/report.txt",
        SkillResourceKind::Asset,
        b"safe",
        b"safe",
    );
    let missing_request = test_request(uri, workspace.path(), "missing/out.txt");
    let missing = SkillResourceMaterializer::new()
        .materialize(&session, &missing_request)
        .unwrap_err();
    assert_eq!(
        missing.code(),
        SkillMaterializationErrorCode::DestinationParentNotFound
    );

    let (tampered_session, tampered_uri) = test_session(
        "assets/tampered.txt",
        SkillResourceKind::Asset,
        b"expected",
        b"tampered",
    );
    let tampered_request = test_request(tampered_uri, workspace.path(), "tampered.txt");
    let tampered = SkillResourceMaterializer::new()
        .materialize(&tampered_session, &tampered_request)
        .unwrap_err();
    assert_eq!(
        tampered.code(),
        SkillMaterializationErrorCode::ResourceError
    );
    assert_eq!(
        tampered.resource_error().unwrap().code().stable_name(),
        "integrityMismatch"
    );
    assert!(!workspace.path().join("tampered.txt").exists());
    assert!(staging_entries(workspace.path()).is_empty());
}

#[test]
fn template_tree_is_published_atomically_and_repeated_as_an_idempotent_hit() {
    let workspace = tempdir().unwrap();
    fs::create_dir(workspace.path().join("exports")).unwrap();
    let budget = b"xlsx-template";
    let notes = b"template notes\n";
    let (session, package) = test_tree_session(&[
        (
            "templates/office/budget.xlsx",
            SkillResourceKind::Other,
            budget,
            budget,
        ),
        (
            "templates/office/docs/notes.txt",
            SkillResourceKind::Other,
            notes,
            notes,
        ),
        (
            "templates/unrelated.txt",
            SkillResourceKind::Other,
            b"not selected",
            b"not selected",
        ),
    ]);
    let request = test_tree_request(
        package,
        "templates/office",
        workspace.path(),
        "exports/office-kit",
    );
    let materializer = SkillResourceMaterializer::new();

    let created = materializer
        .materialize_template_tree(&session, &request)
        .unwrap();
    assert_eq!(created.status(), SkillMaterializationStatus::Created);
    assert_eq!(created.file_count(), 2);
    assert_eq!(created.byte_length(), (budget.len() + notes.len()) as u64);
    assert_eq!(created.bytes_written(), created.byte_length());
    assert!(created
        .plan_digest()
        .starts_with(SKILL_MATERIALIZATION_TREE_DIGEST_PREFIX));
    assert_eq!(
        created
            .entries()
            .iter()
            .map(SkillMaterializedTreeEntry::relative_path)
            .collect::<Vec<_>>(),
        vec!["budget.xlsx", "docs/notes.txt"]
    );
    let destination = workspace.path().join("exports/office-kit");
    assert_eq!(fs::read(destination.join("budget.xlsx")).unwrap(), budget);
    assert_eq!(fs::read(destination.join("docs/notes.txt")).unwrap(), notes);
    assert!(!destination.join("unrelated.txt").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(destination.join("docs"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(destination.join("budget.xlsx"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(staging_entries(&workspace.path().join("exports")).is_empty());

    let repeated = materializer
        .materialize_template_tree(&session, &request)
        .unwrap();
    assert_eq!(
        repeated.status(),
        SkillMaterializationStatus::AlreadyPresent
    );
    assert_eq!(repeated.bytes_written(), 0);
    assert_eq!(repeated.plan_digest(), created.plan_digest());
    assert!(staging_entries(&workspace.path().join("exports")).is_empty());
}

#[test]
fn template_tree_never_merges_with_or_overwrites_an_existing_destination() {
    let workspace = tempdir().unwrap();
    let bytes = b"new-template";
    let (session, package) = test_tree_session(&[(
        "templates/kit/template.txt",
        SkillResourceKind::Other,
        bytes,
        bytes,
    )]);
    let request = test_tree_request(package, "templates/kit", workspace.path(), "kit");
    fs::create_dir(workspace.path().join("kit")).unwrap();
    fs::write(workspace.path().join("kit/canary.txt"), b"CANARY").unwrap();

    let error = SkillResourceMaterializer::new()
        .materialize_template_tree(&session, &request)
        .unwrap_err();
    assert_eq!(error.code(), SkillMaterializationErrorCode::Conflict);
    assert_eq!(
        fs::read(workspace.path().join("kit/canary.txt")).unwrap(),
        b"CANARY"
    );
    assert!(!workspace.path().join("kit/template.txt").exists());
    assert!(staging_entries(workspace.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn template_tree_rejects_nested_symlinks_without_touching_their_targets() {
    use std::os::unix::fs::symlink;

    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let bytes = b"template";
    let (session, package) = test_tree_session(&[(
        "templates/kit/docs/template.txt",
        SkillResourceKind::Other,
        bytes,
        bytes,
    )]);
    let request = test_tree_request(package, "templates/kit", workspace.path(), "kit");
    fs::create_dir(workspace.path().join("kit")).unwrap();
    fs::write(outside.path().join("canary.txt"), b"CANARY").unwrap();
    symlink(outside.path(), workspace.path().join("kit/docs")).unwrap();

    let error = SkillResourceMaterializer::new()
        .materialize_template_tree(&session, &request)
        .unwrap_err();
    assert_eq!(
        error.code(),
        SkillMaterializationErrorCode::UnsafeDestination
    );
    assert_eq!(
        fs::read(outside.path().join("canary.txt")).unwrap(),
        b"CANARY"
    );
    assert!(!outside.path().join("template.txt").exists());
    assert!(staging_entries(workspace.path()).is_empty());
}

#[test]
fn template_tree_validates_prefix_empty_source_and_all_bytes_before_writing() {
    let workspace = tempdir().unwrap();
    let denied = SkillTemplateTreeMaterializationRequest::new(
        SkillPackageUri::new(
            SkillId::parse("installed:user:21234567-89ab-4def-8123-456789abcdef").unwrap(),
            SkillRevision::parse(format!("skill-package-sha256-v3:{}", "c".repeat(64))).unwrap(),
        ),
        SkillResourcePath::parse("assets").unwrap(),
        workspace.path(),
        SkillMaterializationDestination::parse("denied").unwrap(),
    )
    .unwrap_err();
    assert_eq!(
        denied.code(),
        SkillMaterializationErrorCode::SourcePrefixDenied
    );

    let (empty_session, empty_package) = test_tree_session(&[(
        "templates/other/file.txt",
        SkillResourceKind::Other,
        b"other",
        b"other",
    )]);
    let empty_request = test_tree_request(
        empty_package,
        "templates/missing",
        workspace.path(),
        "empty",
    );
    let empty = SkillResourceMaterializer::new()
        .materialize_template_tree(&empty_session, &empty_request)
        .unwrap_err();
    assert_eq!(empty.code(), SkillMaterializationErrorCode::EmptySource);
    assert!(!workspace.path().join("empty").exists());

    let (tampered_session, tampered_package) = test_tree_session(&[
        (
            "templates/kit/a.txt",
            SkillResourceKind::Other,
            b"safe",
            b"safe",
        ),
        (
            "templates/kit/b.txt",
            SkillResourceKind::Other,
            b"expected",
            b"tampered",
        ),
    ]);
    let tampered_request = test_tree_request(
        tampered_package,
        "templates/kit",
        workspace.path(),
        "tampered",
    );
    let tampered = SkillResourceMaterializer::new()
        .materialize_template_tree(&tampered_session, &tampered_request)
        .unwrap_err();
    assert_eq!(
        tampered.code(),
        SkillMaterializationErrorCode::ResourceError
    );
    assert_eq!(
        tampered.resource_error().unwrap().code().stable_name(),
        "integrityMismatch"
    );
    assert!(!workspace.path().join("tampered").exists());
    assert!(staging_entries(workspace.path()).is_empty());
}

#[test]
fn concurrent_identical_template_trees_converge_without_merging() {
    let workspace = tempdir().unwrap();
    let bytes = b"shared-template";
    let (session, package) = test_tree_session(&[(
        "templates/kit/nested/template.txt",
        SkillResourceKind::Other,
        bytes,
        bytes,
    )]);
    let request = test_tree_request(package, "templates/kit", workspace.path(), "kit");
    let session = Arc::new(session);
    let request = Arc::new(request);
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let session = session.clone();
        let request = request.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            SkillResourceMaterializer::new()
                .materialize_template_tree(&session, &request)
                .unwrap()
                .status()
        }));
    }
    let mut statuses = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    statuses.sort_by_key(|status| status.stable_name());
    assert_eq!(
        statuses,
        vec![
            SkillMaterializationStatus::AlreadyPresent,
            SkillMaterializationStatus::Created
        ]
    );
    assert_eq!(
        fs::read(workspace.path().join("kit/nested/template.txt")).unwrap(),
        bytes
    );
    assert!(staging_entries(workspace.path()).is_empty());
}

#[test]
fn concurrent_identical_requests_converge_to_one_file() {
    let workspace = tempdir().unwrap();
    let bytes = b"concurrent bytes";
    let (session, uri) = test_session("assets/shared.txt", SkillResourceKind::Asset, bytes, bytes);
    let request = test_request(uri, workspace.path(), "shared.txt");
    let session = Arc::new(session);
    let request = Arc::new(request);
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let session = session.clone();
        let request = request.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            SkillResourceMaterializer::new()
                .materialize(&session, &request)
                .unwrap()
                .status()
        }));
    }
    let mut statuses = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    statuses.sort_by_key(|status| status.stable_name());
    assert_eq!(
        statuses,
        vec![
            SkillMaterializationStatus::AlreadyPresent,
            SkillMaterializationStatus::Created
        ]
    );
    assert_eq!(
        fs::read(workspace.path().join("shared.txt")).unwrap(),
        bytes
    );
    assert!(staging_entries(workspace.path()).is_empty());
}
