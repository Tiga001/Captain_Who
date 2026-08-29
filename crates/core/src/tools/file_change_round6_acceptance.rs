use super::file_change_staged::{
    abort, append, begin, commit, edit, public_result_projection, status, StagedSource,
};
use super::{ToolExecutionContext, ToolRegistry};
use crate::file_change::{
    content_digest, FileChangeCommitter, FileChangeEdit, FileChangeErrorCode, FileChangeOperation,
    FileChangePathPolicy, FileChangePlan, FileChangePlanner,
};
use crate::protocol::{
    AgentApprovalStatus, AgentCommandPermission, AgentFileChangeProposal, AgentPatchPermission,
    AgentPermissions, AgentProposedAction, AgentReadPermission, AgentRunContext, AgentToolCall,
    AgentToolResult, AgentWorkspaceContext, AgentWritePermission,
};
use crate::storage::models::ChatConversationRecord;
use crate::storage::service::StorageService;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;

const ACCEPTANCE_BASE_ENV: &str = "FILE_CHANGE_ACCEPTANCE_BASE";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileEvidence {
    exists: bool,
    sha256: Option<String>,
    byte_count: Option<u64>,
    mode: Option<u32>,
    inode: Option<u64>,
    mtime_ns: Option<i128>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcceptanceReport {
    schema_version: u32,
    acceptance_root: String,
    direct_canary_digest: String,
    staged_canary_digest: String,
    initial: BTreeMap<String, FileEvidence>,
    final_state: BTreeMap<String, FileEvidence>,
    private_canary_locations: Vec<String>,
    public_canary_hits: u64,
    checks: Vec<String>,
}

struct AcceptanceRoot {
    path: PathBuf,
    _owned: Option<TempDir>,
}

impl AcceptanceRoot {
    fn create() -> Self {
        if let Some(base) = std::env::var_os(ACCEPTANCE_BASE_ENV) {
            let base = PathBuf::from(base);
            assert!(base.is_absolute(), "{ACCEPTANCE_BASE_ENV} must be absolute");
            fs::create_dir_all(&base).expect("create externally supplied acceptance base");
            let path = base.join("acceptance-root");
            assert!(
                !path.exists(),
                "acceptance-root must not pre-exist: {}",
                path.display()
            );
            fs::create_dir(&path).expect("create acceptance-root");
            return Self { path, _owned: None };
        }

        let owned = tempfile::Builder::new()
            .prefix("fc6-")
            .tempdir()
            .expect("create task-specific temporary directory");
        let path = owned.path().join("acceptance-root");
        fs::create_dir(&path).expect("create acceptance-root");
        Self {
            path,
            _owned: Some(owned),
        }
    }
}

#[test]
fn isolated_file_change_acceptance_matrix() {
    let acceptance = AcceptanceRoot::create();
    let workspace_path = acceptance.path.join("workspace");
    let outside_path = acceptance.path.join("outside");
    fs::create_dir(&workspace_path).expect("create workspace");
    fs::create_dir(&outside_path).expect("create outside");
    let workspace = fs::canonicalize(workspace_path).expect("canonicalize workspace");
    let outside = fs::canonicalize(outside_path).expect("canonicalize outside");

    seed_acceptance_files(&workspace, &outside);
    let tracked = tracked_paths(&workspace, &outside);
    let initial = snapshot_paths(&tracked);
    let outside_sentinel_before = evidence(&outside.join("sentinel.txt"));
    let direct_canary = format!("DIRECT_FILE_CHANGE_CANARY_{}", Uuid::new_v4());
    let staged_canary = format!("STAGED_FILE_CHANGE_CANARY_{}", Uuid::new_v4());
    let registry = ToolRegistry::defaults_with_search(None);
    let direct_context = make_context(
        Some(&workspace),
        None,
        "acceptance-direct-conversation",
        Some("acceptance-project"),
        "acceptance-direct-run",
        AgentReadPermission::WorkspaceOnly,
        AgentWritePermission::WorkspaceOnly,
    );
    let mut checks = Vec::new();

    direct_create_and_no_clobber(&registry, &direct_context, &workspace, &direct_canary);
    checks.push("direct create uses a missing observation; create-existing is zero-effect".into());

    direct_update_matrix(&registry, &direct_context, &workspace);
    checks.push(
        "complete update and replace/insert-before/insert-after/append/prepend preserve exact bytes"
            .into(),
    );

    direct_match_delete_reject_and_conflict_matrix(&registry, &direct_context, &workspace);
    checks.push(
        "zero/ambiguous matches, reject, delete errors, stale Base, and create race fail closed"
            .into(),
    );

    path_and_permission_matrix(&workspace, &outside);
    checks.push(
        "workspace/all/denied, traversal, symlink, hard-link, special file, VCS, and Office boundaries"
            .into(),
    );

    let database = acceptance.path.join("acceptance-storage.sqlite");
    let (private_canary_locations, public_canary_hits) =
        staged_matrix(&registry, &workspace, &database, &staged_canary);
    checks.push(
        "128 KiB restart/owner/preview/commit and 1 MiB/4 MiB/4 MiB+1/UTF-8/idempotency boundaries"
            .into(),
    );

    assert_eq!(
        evidence(&outside.join("sentinel.txt")),
        outside_sentinel_before,
        "outside sentinel changed"
    );
    let final_state = snapshot_paths(&tracked);
    let report = AcceptanceReport {
        schema_version: 1,
        acceptance_root: acceptance.path.to_string_lossy().into_owned(),
        direct_canary_digest: content_digest(direct_canary.as_bytes()),
        staged_canary_digest: content_digest(staged_canary.as_bytes()),
        initial,
        final_state,
        private_canary_locations,
        public_canary_hits,
        checks,
    };
    let encoded = serde_json::to_vec_pretty(&report).expect("serialize acceptance evidence");
    assert!(!contains(&encoded, direct_canary.as_bytes()));
    assert!(!contains(&encoded, staged_canary.as_bytes()));
    let report_path = acceptance.path.join("acceptance-report.json");
    fs::write(&report_path, encoded).expect("write isolated acceptance report");
    println!(
        "ROUND6_FILE_CHANGE_ACCEPTANCE_REPORT={}",
        report_path.display()
    );
}

fn seed_acceptance_files(workspace: &Path, outside: &Path) {
    fs::write(
        workspace.join("existing.txt"),
        "HEADER\nreplace-me\nbefore-anchor\nafter-anchor\nTAIL\n",
    )
    .unwrap();
    fs::write(workspace.join("existing-target.txt"), "old target\n").unwrap();
    fs::write(workspace.join("conflict.txt"), "approved base\n").unwrap();
    fs::write(workspace.join("delete-me.txt"), "delete me\n").unwrap();
    fs::write(
        workspace.join("executable.sh"),
        "#!/bin/sh\nprintf 'before\\n'\n",
    )
    .unwrap();
    fs::write(workspace.join("crlf.txt"), b"alpha\r\nanchor\r\nomega\r\n").unwrap();
    fs::write(workspace.join("unicode.txt"), "你好，世界 🙂\n").unwrap();
    fs::write(
        outside.join("sentinel.txt"),
        "OUTSIDE SENTINEL: NEVER CHANGE\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            workspace.join("executable.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
}

fn tracked_paths(workspace: &Path, outside: &Path) -> BTreeMap<String, PathBuf> {
    [
        ("workspace/quick_sort.py", workspace.join("quick_sort.py")),
        ("workspace/existing.txt", workspace.join("existing.txt")),
        (
            "workspace/existing-target.txt",
            workspace.join("existing-target.txt"),
        ),
        ("workspace/conflict.txt", workspace.join("conflict.txt")),
        ("workspace/delete-me.txt", workspace.join("delete-me.txt")),
        ("workspace/executable.sh", workspace.join("executable.sh")),
        ("workspace/crlf.txt", workspace.join("crlf.txt")),
        ("workspace/unicode.txt", workspace.join("unicode.txt")),
        ("outside/sentinel.txt", outside.join("sentinel.txt")),
    ]
    .into_iter()
    .map(|(name, path)| (name.to_string(), path))
    .collect()
}

fn snapshot_paths(paths: &BTreeMap<String, PathBuf>) -> BTreeMap<String, FileEvidence> {
    paths
        .iter()
        .map(|(name, path)| (name.clone(), evidence(path)))
        .collect()
}

fn evidence(path: &Path) -> FileEvidence {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return FileEvidence {
            exists: false,
            sha256: None,
            byte_count: None,
            mode: None,
            inode: None,
            mtime_ns: None,
        };
    };
    let bytes = metadata
        .is_file()
        .then(|| fs::read(path).expect("read evidence file"));
    let sha256 = bytes.as_ref().map(|bytes| hex_sha256(bytes));
    #[cfg(unix)]
    let (mode, inode, mtime_ns) = {
        use std::os::unix::fs::MetadataExt;
        (
            Some(metadata.mode() & 0o7777),
            Some(metadata.ino()),
            Some(i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec())),
        )
    };
    #[cfg(not(unix))]
    let (mode, inode, mtime_ns) = {
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| i128::try_from(duration.as_nanos()).unwrap_or(i128::MAX));
        (
            Some(u32::from(metadata.permissions().readonly())),
            None,
            modified,
        )
    };
    FileEvidence {
        exists: true,
        sha256,
        byte_count: bytes.as_ref().map(|bytes| bytes.len() as u64),
        mode,
        inode,
        mtime_ns,
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn make_context(
    workspace: Option<&Path>,
    storage: Option<Arc<StorageService>>,
    conversation_id: &str,
    project_id: Option<&str>,
    run_id: &str,
    read: AgentReadPermission,
    write: AgentWritePermission,
) -> ToolExecutionContext {
    ToolExecutionContext::from_run_context(Some(&AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(conversation_id.to_string()),
        project_id: project_id.map(ToString::to_string),
        workspace: workspace.map(|root| AgentWorkspaceContext {
            project_id: project_id.map(ToString::to_string),
            display_name: Some("round6-acceptance".to_string()),
            root_path: Some(root.to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions {
            read,
            write,
            command: AgentCommandPermission::RequireApproval,
            command_safety: Default::default(),
            patch: AgentPatchPermission::RequireApproval,
            builtin_execution: Default::default(),
        },
    }))
    .with_runtime_services(run_id.to_string(), storage)
    .with_file_change_tool_set_revision("round6-tool-set-v1".to_string())
    .with_file_change_provider_wire_revision("round6-provider-wire-v1".to_string())
}

fn observe(registry: &ToolRegistry, context: &ToolExecutionContext, path: &str) -> String {
    let call = AgentToolCall {
        id: format!("read-{}", Uuid::new_v4()),
        tool: "read_file".to_string(),
        args: json!({"path": path}),
        approval_status: AgentApprovalStatus::Approved,
        reason: None,
    };
    let result = registry.execute(context, &call);
    assert!(result.ok, "read_file failed: {:?}", result.error);
    result.result.unwrap()["observationId"]
        .as_str()
        .expect("read_file observationId")
        .to_string()
}

fn build_direct_proposal(
    registry: &ToolRegistry,
    context: &ToolExecutionContext,
    args: Value,
) -> Result<(AgentToolCall, AgentFileChangeProposal), crate::protocol::AgentError> {
    let call = AgentToolCall {
        id: format!("apply-{}", Uuid::new_v4()),
        tool: "apply_patch".to_string(),
        args: json!({ "request": args }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let action = registry.proposed_action(context, &call)?;
    let AgentProposedAction::FileChange { file_change } = action else {
        panic!("apply_patch must produce FileChange")
    };
    Ok((call, file_change))
}

fn commit_proposal(
    workspace: Option<&Path>,
    allow_outside_workspace: bool,
    mut proposal: AgentFileChangeProposal,
    now: u64,
) {
    let target = FileChangePathPolicy::new(workspace, allow_outside_workspace)
        .resolve(&proposal.execution.canonical_target)
        .expect("resolve frozen canonical target");
    let plan = FileChangePlan::from_binding(&proposal.execution).expect("rebuild frozen plan");
    let mut journal = proposal.execution.delete_journal.take();
    let committed_at = journal
        .as_ref()
        .map(|journal| now.max(journal.created_at))
        .unwrap_or(now);
    let committed = FileChangeCommitter
        .commit_fresh(
            &proposal.transaction_id,
            &target,
            &plan,
            committed_at,
            journal.as_mut(),
        )
        .expect("commit frozen FileChange");
    committed
        .receipt
        .validate()
        .expect("valid FileChange receipt");
    if let Some(journal) = journal.as_mut() {
        FileChangeCommitter
            .finalize_delete(&target, journal, committed_at.saturating_add(1))
            .expect("finalize recoverable delete");
    }
}

fn direct_create_and_no_clobber(
    registry: &ToolRegistry,
    context: &ToolExecutionContext,
    workspace: &Path,
    direct_canary: &str,
) {
    let observation = observe(registry, context, "quick_sort.py");
    let content = format!(
        "# {direct_canary}\ndef quick_sort(values):\n    if len(values) < 2:\n        return values[:]\n    pivot = values[len(values) // 2]\n    return quick_sort([v for v in values if v < pivot]) + [v for v in values if v == pivot] + quick_sort([v for v in values if v > pivot])\n"
    );
    let args = json!({
        "action":"apply", "operation":"create", "filePath":"quick_sort.py",
        "observationId":observation, "content":content
    });
    let call = AgentToolCall {
        id: format!("apply-{}", Uuid::new_v4()),
        tool: "apply_patch".to_string(),
        args: json!({ "request": args.clone() }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let projected = registry.event_call_projection(&call);
    assert!(!projected.args.to_string().contains(direct_canary));
    let (_, proposal) = build_direct_proposal(registry, context, args).unwrap();
    commit_proposal(Some(workspace), false, proposal, 10);
    assert_eq!(
        fs::read_to_string(workspace.join("quick_sort.py")).unwrap(),
        content
    );

    let before = evidence(&workspace.join("existing.txt"));
    let observation = observe(registry, context, "existing.txt");
    let error = build_direct_proposal(
        registry,
        context,
        json!({
            "action":"apply", "operation":"create", "filePath":"existing.txt",
            "observationId":observation, "content":"must not overwrite\n"
        }),
    )
    .unwrap_err();
    assert_eq!(error.code(), Some("agent.apply_patch.file_exists"));
    assert_eq!(error.to_string(), "文件已存在。");
    assert!(!error.to_string().contains("structured_edit_error"));
    assert_eq!(evidence(&workspace.join("existing.txt")), before);
}

fn direct_update_matrix(registry: &ToolRegistry, context: &ToolExecutionContext, workspace: &Path) {
    let observation = observe(registry, context, "existing-target.txt");
    let (_, proposal) = build_direct_proposal(
        registry,
        context,
        json!({
            "action":"apply", "operation":"update", "filePath":"existing-target.txt",
            "observationId":observation, "content":"complete target replacement\n"
        }),
    )
    .unwrap();
    commit_proposal(Some(workspace), false, proposal, 20);
    assert_eq!(
        fs::read_to_string(workspace.join("existing-target.txt")).unwrap(),
        "complete target replacement\n"
    );

    let observation = observe(registry, context, "existing.txt");
    let (_, proposal) = build_direct_proposal(
        registry,
        context,
        json!({
            "action":"apply", "operation":"update", "filePath":"existing.txt",
            "observationId":observation,
            "edits":[
                {"kind":"prepend","text":"PRELUDE\n"},
                {"kind":"replace","oldText":"replace-me","newText":"replaced"},
                {"kind":"insert_before","anchor":"before-anchor","text":"BEFORE\n"},
                {"kind":"insert_after","anchor":"after-anchor","text":"\nAFTER"},
                {"kind":"append","text":"EPILOGUE\n"}
            ]
        }),
    )
    .unwrap();
    commit_proposal(Some(workspace), false, proposal, 21);
    assert_eq!(
        fs::read_to_string(workspace.join("existing.txt")).unwrap(),
        "PRELUDE\nHEADER\nreplaced\nBEFORE\nbefore-anchor\nafter-anchor\nAFTER\nTAIL\nEPILOGUE\n"
    );

    let executable = workspace.join("executable.sh");
    let mode_before = evidence(&executable).mode;
    let observation = observe(registry, context, "executable.sh");
    let (_, proposal) = build_direct_proposal(
        registry,
        context,
        json!({
            "action":"apply", "operation":"update", "filePath":"executable.sh",
            "observationId":observation,
            "edits":[{"kind":"append","text":"# preserved executable mode\n"}]
        }),
    )
    .unwrap();
    commit_proposal(Some(workspace), false, proposal, 22);
    assert_eq!(evidence(&executable).mode, mode_before);

    let observation = observe(registry, context, "crlf.txt");
    let (_, proposal) = build_direct_proposal(
        registry,
        context,
        json!({
            "action":"apply", "operation":"update", "filePath":"crlf.txt",
            "observationId":observation,
            "edits":[
                {"kind":"insert_before","anchor":"anchor\r\n","text":"before\r\n"},
                {"kind":"append","text":"tail\r\n"}
            ]
        }),
    )
    .unwrap();
    commit_proposal(Some(workspace), false, proposal, 23);
    let crlf = fs::read(workspace.join("crlf.txt")).unwrap();
    assert_eq!(crlf, b"alpha\r\nbefore\r\nanchor\r\nomega\r\ntail\r\n");
    assert!(crlf
        .iter()
        .enumerate()
        .all(|(index, byte)| *byte != b'\n' || index > 0 && crlf[index - 1] == b'\r'));

    let observation = observe(registry, context, "unicode.txt");
    let (_, proposal) = build_direct_proposal(
        registry,
        context,
        json!({
            "action":"apply", "operation":"update", "filePath":"unicode.txt",
            "observationId":observation,
            "edits":[{"kind":"replace","oldText":"世界 🙂","newText":"世界 🌌🚀"}]
        }),
    )
    .unwrap();
    commit_proposal(Some(workspace), false, proposal, 24);
    assert_eq!(
        fs::read_to_string(workspace.join("unicode.txt")).unwrap(),
        "你好，世界 🌌🚀\n"
    );
}

fn direct_match_delete_reject_and_conflict_matrix(
    registry: &ToolRegistry,
    context: &ToolExecutionContext,
    workspace: &Path,
) {
    fs::write(workspace.join("matches.txt"), "same\nunique\nsame\n").unwrap();
    let before = evidence(&workspace.join("matches.txt"));
    let observation = observe(registry, context, "matches.txt");
    let missing = build_direct_proposal(
        registry,
        context,
        json!({
            "action":"apply", "operation":"update", "filePath":"matches.txt",
            "observationId":observation,
            "edits":[{"kind":"replace","oldText":"absent","newText":"x"}]
        }),
    )
    .unwrap_err();
    assert_eq!(missing.code(), Some("agent.apply_patch.match_not_found"));
    let ambiguous = build_direct_proposal(
        registry,
        context,
        json!({
            "action":"apply", "operation":"update", "filePath":"matches.txt",
            "observationId":observation,
            "edits":[{"kind":"replace","oldText":"same","newText":"x"}]
        }),
    )
    .unwrap_err();
    assert_eq!(ambiguous.code(), Some("agent.apply_patch.ambiguous_match"));
    assert_eq!(evidence(&workspace.join("matches.txt")), before);

    let rejected_before = evidence(&workspace.join("conflict.txt"));
    let parent_before = evidence(workspace);
    let observation = observe(registry, context, "conflict.txt");
    let _frozen_but_rejected = build_direct_proposal(
        registry,
        context,
        json!({
            "action":"apply", "operation":"update", "filePath":"conflict.txt",
            "observationId":observation, "content":"rejected target\n"
        }),
    )
    .unwrap();
    assert_eq!(evidence(&workspace.join("conflict.txt")), rejected_before);
    assert_eq!(evidence(workspace), parent_before);

    let observation = observe(registry, context, "conflict.txt");
    let (_, stale_proposal) = build_direct_proposal(
        registry,
        context,
        json!({
            "action":"apply", "operation":"update", "filePath":"conflict.txt",
            "observationId":observation, "content":"approved target\n"
        }),
    )
    .unwrap();
    fs::write(workspace.join("conflict.txt"), "external update wins\n").unwrap();
    let target = FileChangePathPolicy::new(Some(workspace), false)
        .resolve(&stale_proposal.execution.canonical_target)
        .unwrap();
    let plan = FileChangePlan::from_binding(&stale_proposal.execution).unwrap();
    let error = FileChangeCommitter
        .commit_fresh(&stale_proposal.transaction_id, &target, &plan, 30, None)
        .unwrap_err();
    assert_eq!(error.code(), FileChangeErrorCode::RevisionConflict);
    assert_eq!(
        fs::read_to_string(workspace.join("conflict.txt")).unwrap(),
        "external update wins\n"
    );

    let observation = observe(registry, context, "raced-create.txt");
    let (_, raced_create) = build_direct_proposal(
        registry,
        context,
        json!({
            "action":"apply", "operation":"create", "filePath":"raced-create.txt",
            "observationId":observation, "content":"approved create\n"
        }),
    )
    .unwrap();
    fs::write(workspace.join("raced-create.txt"), "external create wins\n").unwrap();
    let target = FileChangePathPolicy::new(Some(workspace), false)
        .resolve(&raced_create.execution.canonical_target)
        .unwrap();
    let plan = FileChangePlan::from_binding(&raced_create.execution).unwrap();
    let error = FileChangeCommitter
        .commit_fresh(&raced_create.transaction_id, &target, &plan, 31, None)
        .unwrap_err();
    assert_eq!(error.code(), FileChangeErrorCode::FileExists);
    assert_eq!(
        fs::read_to_string(workspace.join("raced-create.txt")).unwrap(),
        "external create wins\n"
    );

    let observation = observe(registry, context, "delete-me.txt");
    let (_, proposal) = build_direct_proposal(
        registry,
        context,
        json!({
            "action":"apply", "operation":"delete", "filePath":"delete-me.txt",
            "observationId":observation
        }),
    )
    .unwrap();
    commit_proposal(Some(workspace), false, proposal, 32);
    assert!(!workspace.join("delete-me.txt").exists());

    let observation = observe(registry, context, "missing-delete.txt");
    let error = build_direct_proposal(
        registry,
        context,
        json!({
            "action":"apply", "operation":"delete", "filePath":"missing-delete.txt",
            "observationId":observation
        }),
    )
    .unwrap_err();
    assert_eq!(error.code(), Some("agent.apply_patch.file_missing"));
}

fn path_and_permission_matrix(workspace: &Path, outside: &Path) {
    fs::create_dir(workspace.join("directory")).unwrap();
    for vcs in [".git", ".hg", ".svn"] {
        fs::create_dir(workspace.join(vcs)).unwrap();
    }
    let workspace_only = FileChangePathPolicy::new(Some(workspace), false);
    let write_all = FileChangePathPolicy::new(Some(workspace), true);
    let denied = FileChangePathPolicy::new(None, false);
    assert!(workspace_only.resolve("safe.txt").is_ok());
    assert_code(
        workspace_only.resolve(outside.join("escape.txt").to_str().unwrap()),
        FileChangeErrorCode::PermissionDenied,
    );
    assert!(write_all
        .resolve(outside.join("allowed.txt").to_str().unwrap())
        .is_ok());
    assert_code(
        denied.resolve(outside.join("denied.txt").to_str().unwrap()),
        FileChangeErrorCode::PermissionDenied,
    );
    assert_code(
        workspace_only.resolve("../outside/sentinel.txt"),
        FileChangeErrorCode::PermissionDenied,
    );
    assert_code(
        workspace_only.resolve("directory"),
        FileChangeErrorCode::NotRegularFile,
    );
    for vcs in [".git/file.txt", ".hg/file.txt", ".svn/file.txt"] {
        assert_code(
            workspace_only.resolve(vcs),
            FileChangeErrorCode::PermissionDenied,
        );
    }
    for office in ["file.docx", "file.xlsx", "file.pptx", "file.pdf"] {
        assert_code(
            workspace_only.resolve(office),
            FileChangeErrorCode::UnsupportedFileType,
        );
    }
    assert_code(
        FileChangePathPolicy::new(None, true).resolve("relative.txt"),
        FileChangeErrorCode::WorkspaceRequired,
    );

    let absolute_target = outside.join("absolute-create.txt");
    let resolved = FileChangePathPolicy::new(None, true)
        .resolve(absolute_target.to_str().unwrap())
        .unwrap();
    let plan = FileChangePlanner
        .plan(crate::file_change::FileChangePlanRequest {
            operation: FileChangeOperation::Create,
            file_path: resolved.display_path(),
            base: crate::file_change::FileChangeBase::Missing,
            mutation: crate::file_change::FileChangeMutation::Complete("absolute success\n".into()),
        })
        .unwrap();
    FileChangeCommitter
        .commit_fresh("round6-absolute", &resolved, &plan, 40, None)
        .unwrap();
    assert_eq!(
        fs::read_to_string(absolute_target).unwrap(),
        "absolute success\n"
    );

    let home = crate::expand_system_path("@home").unwrap().unwrap();
    assert!(home.is_absolute());
    assert!(crate::expand_system_path("@desktop/../escape").is_err());

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;

        symlink(outside, workspace.join("ancestor-link")).unwrap();
        symlink(
            outside.join("sentinel.txt"),
            workspace.join("leaf-link.txt"),
        )
        .unwrap();
        fs::hard_link(
            outside.join("sentinel.txt"),
            workspace.join("hard-link.txt"),
        )
        .unwrap();
        let fifo = workspace.join("fifo");
        let fifo_c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        let socket = workspace.join("s");
        let _listener = UnixListener::bind(&socket).unwrap();

        assert_code(
            workspace_only.resolve("ancestor-link/new.txt"),
            FileChangeErrorCode::SymlinkForbidden,
        );
        assert_code(
            workspace_only.resolve("leaf-link.txt"),
            FileChangeErrorCode::SymlinkForbidden,
        );
        assert_code(
            workspace_only.resolve("hard-link.txt"),
            FileChangeErrorCode::HardLinkForbidden,
        );
        assert_code(
            workspace_only.resolve("fifo"),
            FileChangeErrorCode::NotRegularFile,
        );
        assert_code(
            workspace_only.resolve("s"),
            FileChangeErrorCode::NotRegularFile,
        );
    }
}

fn staged_matrix(
    registry: &ToolRegistry,
    workspace: &Path,
    database: &Path,
    staged_canary: &str,
) -> (Vec<String>, u64) {
    let conversation = "acceptance-staged-conversation";
    let project = Some("acceptance-project");
    let run = "acceptance-staged-run";
    let storage = Arc::new(StorageService::open(database).unwrap());
    save_conversation(&storage, conversation);
    let context = make_context(
        Some(workspace),
        Some(storage.clone()),
        conversation,
        project,
        run,
        AgentReadPermission::WorkspaceOnly,
        AgentWritePermission::WorkspaceOnly,
    );
    let observation = observe(registry, &context, "large.md");
    let begun = begin(
        &call_context(&context, "begin-large"),
        StagedSource::new("apply_patch", content_digest(b"begin-large")),
        FileChangeOperation::Create,
        None,
        "large.md".to_string(),
        observation,
        None,
    )
    .unwrap();
    let transaction_id = begun["transactionId"].as_str().unwrap().to_string();
    let content = exact_size_text(128 * 1024, &format!("{staged_canary}\n"));
    let split = 64 * 1024;
    let first = content[..split].to_string();
    let second = content[split..].to_string();
    let append_digest = content_digest(first.as_bytes());
    let first_receipt = append(
        &call_context(&context, "append-large-0"),
        "apply_patch",
        transaction_id.clone(),
        0,
        0,
        first.clone(),
        append_digest.clone(),
    )
    .unwrap();
    let replay = append(
        &call_context(&context, "append-large-0-retry"),
        "apply_patch",
        transaction_id.clone(),
        0,
        0,
        first,
        append_digest,
    )
    .unwrap();
    assert_eq!(replay, first_receipt);
    let mismatch = append(
        &call_context(&context, "append-large-0-mismatch"),
        "apply_patch",
        transaction_id.clone(),
        0,
        0,
        "different".to_string(),
        content_digest(b"different"),
    )
    .unwrap_err();
    assert_eq!(mismatch.code(), Some("agent.apply_patch.replay_mismatch"));
    let out_of_order = append(
        &call_context(&context, "append-large-out-of-order"),
        "apply_patch",
        transaction_id.clone(),
        2,
        1,
        "x".to_string(),
        content_digest(b"out-of-order"),
    )
    .unwrap_err();
    assert_eq!(
        out_of_order.code(),
        Some("agent.apply_patch.mutation_out_of_order")
    );
    let stale = append(
        &call_context(&context, "append-large-stale"),
        "apply_patch",
        transaction_id.clone(),
        1,
        0,
        "x".to_string(),
        content_digest(b"stale"),
    )
    .unwrap_err();
    assert_eq!(
        stale.code(),
        Some("agent.apply_patch.draft_revision_conflict")
    );

    let raw_result = AgentToolResult {
        exact_archive_file: None,
        call_id: "append-large-0".to_string(),
        tool: "apply_patch".to_string(),
        ok: true,
        result: Some(first_receipt),
        error: None,
    };
    assert!(
        !serde_json::to_string(&public_result_projection(&raw_result))
            .unwrap()
            .contains(staged_canary)
    );

    drop(context);
    drop(storage);
    let recovered_storage = Arc::new(StorageService::open(database).unwrap());
    let recovered = make_context(
        Some(workspace),
        Some(recovered_storage.clone()),
        conversation,
        project,
        run,
        AgentReadPermission::WorkspaceOnly,
        AgentWritePermission::WorkspaceOnly,
    );
    let foreign = make_context(
        Some(workspace),
        Some(recovered_storage.clone()),
        conversation,
        project,
        "different-run",
        AgentReadPermission::WorkspaceOnly,
        AgentWritePermission::WorkspaceOnly,
    );
    let owner_error = status(&foreign, "apply_patch", transaction_id.clone()).unwrap_err();
    assert_eq!(
        owner_error.code(),
        Some("agent.apply_patch.transaction_owner_mismatch")
    );
    let second_receipt = append(
        &call_context(&recovered, "append-large-1"),
        "apply_patch",
        transaction_id.clone(),
        1,
        1,
        second,
        content_digest(b"append-large-1"),
    )
    .unwrap();
    assert_eq!(second_receipt["byteCount"], 128 * 1024);
    let edited = edit(
        &call_context(&recovered, "edit-large-2"),
        "apply_patch",
        transaction_id.clone(),
        2,
        2,
        vec![FileChangeEdit::Append {
            text: "\n终点 🚀\n".to_string(),
        }],
        content_digest(b"edit-large-2"),
    )
    .unwrap();
    assert_eq!(edited["draftRevision"], 3);
    let current = status(&recovered, "apply_patch", transaction_id.clone()).unwrap();
    assert_eq!(current["nextIndex"], 3);
    let commit_call = AgentToolCall {
        id: "commit-large".to_string(),
        tool: "apply_patch".to_string(),
        args: json!({
            "request": {
                "action":"commit", "transactionId":transaction_id,
                "expectedDraftRevision":3, "summary":"Round 6 isolated large-file acceptance"
            }
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let frozen = commit(
        &call_context(&recovered, "commit-large"),
        &commit_call,
        "apply_patch",
        transaction_id,
        3,
        Some("Round 6 isolated large-file acceptance".to_string()),
    )
    .unwrap();
    commit_proposal(Some(workspace), false, frozen, 100);
    let final_large = fs::read_to_string(workspace.join("large.md")).unwrap();
    assert_eq!(final_large, format!("{content}\n终点 🚀\n"));

    staged_size_boundary(
        registry,
        workspace,
        recovered_storage.clone(),
        conversation,
        project,
        run,
        "one-mib.md",
        vec!["a".repeat(1024 * 1024)],
        false,
    );
    staged_size_boundary(
        registry,
        workspace,
        recovered_storage.clone(),
        conversation,
        project,
        run,
        "four-mib.md",
        vec!["b".repeat(1024 * 1024); 4],
        true,
    );
    let unicode_chunk = exact_utf8_chunk(1024 * 1024);
    staged_size_boundary(
        registry,
        workspace,
        recovered_storage.clone(),
        conversation,
        project,
        run,
        "unicode-boundary.md",
        vec![unicode_chunk],
        false,
    );

    let abort_observation = observe(registry, &recovered, "abort.md");
    let aborted = begin(
        &call_context(&recovered, "begin-abort"),
        StagedSource::new("apply_patch", content_digest(b"begin-abort")),
        FileChangeOperation::Create,
        None,
        "abort.md".to_string(),
        abort_observation,
        None,
    )
    .unwrap();
    let abort_id = aborted["transactionId"].as_str().unwrap().to_string();
    append(
        &call_context(&recovered, "append-abort"),
        "apply_patch",
        abort_id.clone(),
        0,
        0,
        "draft only".to_string(),
        content_digest(b"append-abort"),
    )
    .unwrap();
    assert!(!workspace.join("abort.md").exists());
    let aborted = abort(&recovered, "apply_patch", abort_id.clone()).unwrap();
    assert_eq!(aborted["status"], "aborted");
    assert!(!workspace.join("abort.md").exists());
    assert!(append(
        &call_context(&recovered, "append-after-abort"),
        "apply_patch",
        abort_id,
        1,
        1,
        "no".to_string(),
        content_digest(b"append-after-abort"),
    )
    .is_err());

    drop(foreign);
    drop(recovered);
    drop(recovered_storage);
    let locations = sqlite_canary_locations(database, staged_canary);
    assert_eq!(locations, vec!["agent_file_changes.content".to_string()]);
    let public_hits = public_storage_canary_hits(database, staged_canary);
    assert_eq!(public_hits, 0);
    (locations, public_hits)
}

#[allow(clippy::too_many_arguments)]
fn staged_size_boundary(
    registry: &ToolRegistry,
    workspace: &Path,
    storage: Arc<StorageService>,
    conversation: &str,
    project: Option<&str>,
    run: &str,
    path: &str,
    chunks: Vec<String>,
    assert_plus_one_rejected: bool,
) {
    let context = make_context(
        Some(workspace),
        Some(storage),
        conversation,
        project,
        run,
        AgentReadPermission::WorkspaceOnly,
        AgentWritePermission::WorkspaceOnly,
    );
    let observation = observe(registry, &context, path);
    let begin_call = format!("begin-{path}");
    let begun = begin(
        &call_context(&context, &begin_call),
        StagedSource::new("apply_patch", content_digest(begin_call.as_bytes())),
        FileChangeOperation::Create,
        None,
        path.to_string(),
        observation,
        None,
    )
    .unwrap();
    let transaction_id = begun["transactionId"].as_str().unwrap().to_string();
    let mut revision = 0_u64;
    for (index, chunk) in chunks.into_iter().enumerate() {
        let call_id = format!("append-{path}-{index}");
        let receipt = append(
            &call_context(&context, &call_id),
            "apply_patch",
            transaction_id.clone(),
            index as u64,
            revision,
            chunk,
            content_digest(call_id.as_bytes()),
        )
        .unwrap();
        revision = receipt["draftRevision"].as_u64().unwrap();
    }
    if assert_plus_one_rejected {
        let before = status(&context, "apply_patch", transaction_id.clone()).unwrap();
        assert_eq!(before["byteCount"], 4 * 1024 * 1024);
        let error = append(
            &call_context(&context, "append-four-mib-plus-one"),
            "apply_patch",
            transaction_id.clone(),
            revision,
            revision,
            "x".to_string(),
            content_digest(b"append-four-mib-plus-one"),
        )
        .unwrap_err();
        assert_eq!(error.code(), Some("agent.apply_patch.content_too_large"));
        assert_eq!(
            status(&context, "apply_patch", transaction_id.clone()).unwrap()["byteCount"],
            4 * 1024 * 1024
        );
    }
    abort(&context, "apply_patch", transaction_id).unwrap();
    assert!(!workspace.join(path).exists());
}

fn save_conversation(storage: &StorageService, id: &str) {
    storage
        .save_conversation(ChatConversationRecord {
            id: id.to_string(),
            project_id: None,
            model_id: None,
            title: "Round 6 FileChange acceptance".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
}

fn call_context(context: &ToolExecutionContext, call_id: &str) -> ToolExecutionContext {
    context.clone().with_tool_call_id(call_id.to_string())
}

fn exact_size_text(byte_count: usize, prefix: &str) -> String {
    assert!(prefix.len() <= byte_count);
    let mut text = prefix.to_string();
    text.push_str(&"x".repeat(byte_count - text.len()));
    assert_eq!(text.len(), byte_count);
    text
}

fn exact_utf8_chunk(byte_count: usize) -> String {
    let unit = "你🙂";
    let repeats = byte_count / unit.len();
    let mut chunk = unit.repeat(repeats);
    chunk.push_str(&"u".repeat(byte_count - chunk.len()));
    assert_eq!(chunk.len(), byte_count);
    chunk
}

fn sqlite_canary_locations(database: &Path, canary: &str) -> Vec<String> {
    let connection = rusqlite::Connection::open(database).unwrap();
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap();
    let tables = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    drop(statement);
    let mut matches = Vec::new();
    for table in tables {
        if table.starts_with("sqlite_") {
            continue;
        }
        let table_name = quote_identifier(&table);
        let mut columns_statement = connection
            .prepare(&format!("PRAGMA table_info({table_name})"))
            .unwrap();
        let columns = columns_statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        drop(columns_statement);
        for column in columns {
            let column_name = quote_identifier(&column);
            let count: u64 = connection
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM {table_name} WHERE instr(CAST({column_name} AS TEXT), ?1) > 0"
                    ),
                    [canary],
                    |row| row.get(0),
                )
                .unwrap();
            if count > 0 {
                matches.push(format!("{table}.{column}"));
            }
        }
    }
    matches
}

fn public_storage_canary_hits(database: &Path, canary: &str) -> u64 {
    let connection = rusqlite::Connection::open(database).unwrap();
    [
        ("messages", "content"),
        ("messages", "agent_run_json"),
        ("messages", "ui_state_json"),
        ("conversation_turn_traces", "trace_json"),
        ("conversation_model_context_logs", "item_json"),
        ("conversation_history_blobs", "content"),
    ]
    .into_iter()
    .map(|(table, column)| {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        if !exists {
            return 0;
        }
        connection
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM {} WHERE instr(CAST({} AS TEXT), ?1) > 0",
                    quote_identifier(table),
                    quote_identifier(column)
                ),
                [canary],
                |row| row.get::<_, u64>(0),
            )
            .unwrap_or(0)
    })
    .sum()
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn assert_code<T: std::fmt::Debug>(
    result: Result<T, crate::file_change::FileChangeError>,
    expected: FileChangeErrorCode,
) {
    assert_eq!(result.unwrap_err().code(), expected);
}
