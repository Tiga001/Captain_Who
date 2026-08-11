use super::{
    AgentCommandExecutionResult, AgentCommandPublishedOutput, AgentCommandPublishedOutputKind,
    ManagedCommandOutputCapture,
};
use crate::storage::managed_artifact_repository::ManagedArtifactKind;
use crate::storage::service::{ManagedArtifactAuthority, StorageService};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub const MAX_MANAGED_COMMAND_OUTPUT_FILES: usize = 32;
/// Safety bound for the complete reusable Run workspace. The smaller publication limits apply
/// only to files created or changed by one command.
pub const MAX_MANAGED_COMMAND_OUTPUT_ENTRIES: usize = 1_024;
pub const MAX_MANAGED_COMMAND_OUTPUT_DEPTH: usize = 8;
pub const MAX_MANAGED_COMMAND_OUTPUT_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_MANAGED_COMMAND_OUTPUT_WORKSPACE_BYTES: u64 = 1024 * 1024 * 1024;

/// Publishes every supported file below a trusted managed-command output root.
///
/// This boundary is intentionally independent of PDF command parsing and runtime selection. The
/// presence of a backend-owned capture proves that the caller already selected the trusted
/// managed path. The scan itself grants no command, filesystem, or Skill authority.
pub fn publish_managed_command_outputs(
    storage: &StorageService,
    execution: &mut AgentCommandExecutionResult,
    conversation_id: &str,
    run_id: &str,
    call_id: &str,
) -> Result<(), String> {
    if !command_succeeded(execution) {
        execution.outputs.clear();
        return Ok(());
    }
    let Some(capture) = execution.managed_outputs.as_ref() else {
        return Ok(());
    };
    let files = scan_output_files(capture)?;
    let authority = ManagedArtifactAuthority {
        conversation_id,
        run_id,
        call_id,
    };
    let mut outputs = Vec::with_capacity(files.len());
    for file in files {
        let published = storage.publish_managed_artifact_file(&file.absolute_path, authority)?;
        let kind = match published.kind {
            ManagedArtifactKind::Image => AgentCommandPublishedOutputKind::Image,
            ManagedArtifactKind::Document => AgentCommandPublishedOutputKind::Document,
        };
        outputs.push(AgentCommandPublishedOutput {
            name: file.relative_name,
            kind,
            read_path: published.read_path(),
            mime_type: published.media_type,
            size_bytes: published.size_bytes,
            sha256: published.sha256,
            width: published.width,
            height: published.height,
        });
    }
    execution.outputs = outputs;
    Ok(())
}

/// Captures the immutable comparison baseline immediately before one managed command starts.
/// The command runtime stores this map in `ManagedCommandOutputCapture`; publication then emits
/// only files created or changed by that command, while the run-scoped workspace remains reusable.
pub fn snapshot_managed_command_output_baseline(
    root: &Path,
) -> Result<BTreeMap<String, String>, String> {
    let capture = BorrowedOutputRoot(root);
    let files = scan_output_root(capture.0)?;
    Ok(files
        .into_iter()
        .map(|file| (file.relative_name, file.sha256))
        .collect())
}

fn command_succeeded(execution: &AgentCommandExecutionResult) -> bool {
    execution.exit_code == Some(0)
        && !execution.timed_out
        && !execution.cancelled
        && execution.error.is_none()
}

#[derive(Debug)]
struct ScannedOutput {
    relative_name: String,
    absolute_path: PathBuf,
    sha256: String,
    size_bytes: u64,
}

fn scan_output_files(capture: &ManagedCommandOutputCapture) -> Result<Vec<ScannedOutput>, String> {
    let files = scan_output_root(capture.root())?
        .into_iter()
        .filter(|file| {
            capture
                .baseline()
                .get(&file.relative_name)
                .is_none_or(|digest| digest != &file.sha256)
        })
        .collect::<Vec<_>>();
    if files.len() > MAX_MANAGED_COMMAND_OUTPUT_FILES {
        return Err(format!(
            "managed command produced more than {MAX_MANAGED_COMMAND_OUTPUT_FILES} publishable files"
        ));
    }
    let total_bytes = files.iter().try_fold(0_u64, |total, file| {
        total
            .checked_add(file.size_bytes)
            .ok_or_else(|| "managed output byte count overflowed".to_string())
    })?;
    if total_bytes > MAX_MANAGED_COMMAND_OUTPUT_TOTAL_BYTES {
        return Err(format!(
            "managed command produced more than {MAX_MANAGED_COMMAND_OUTPUT_TOTAL_BYTES} publishable bytes"
        ));
    }
    Ok(files)
}

struct BorrowedOutputRoot<'a>(&'a Path);

fn scan_output_root(root: &Path) -> Result<Vec<ScannedOutput>, String> {
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("managed output directory is unavailable: {error}"))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.file_type().is_dir() {
        return Err("managed output root must be a non-symlink directory".to_string());
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("managed output directory cannot be verified: {error}"))?;
    let mut state = ScanState::default();
    scan_directory(&canonical_root, &canonical_root, 0, &mut state)?;
    state
        .files
        .sort_by(|left, right| left.relative_name.cmp(&right.relative_name));
    Ok(state.files)
}

#[derive(Default)]
struct ScanState {
    entries: usize,
    total_bytes: u64,
    files: Vec<ScannedOutput>,
}

fn scan_directory(
    root: &Path,
    directory: &Path,
    depth: usize,
    state: &mut ScanState,
) -> Result<(), String> {
    if depth > MAX_MANAGED_COMMAND_OUTPUT_DEPTH {
        return Err(format!(
            "managed output nesting exceeds the {MAX_MANAGED_COMMAND_OUTPUT_DEPTH} level limit"
        ));
    }
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("managed output directory cannot be scanned: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("managed output directory cannot be scanned: {error}"))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        state.entries = state.entries.saturating_add(1);
        if state.entries > MAX_MANAGED_COMMAND_OUTPUT_ENTRIES {
            return Err(format!(
                "managed output scan exceeds the {MAX_MANAGED_COMMAND_OUTPUT_ENTRIES} entry limit"
            ));
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("managed output entry cannot be inspected: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err("managed output directories cannot contain symbolic links".to_string());
        }
        if metadata.file_type().is_dir() {
            let canonical = path
                .canonicalize()
                .map_err(|error| format!("managed output directory cannot be verified: {error}"))?;
            if !canonical.starts_with(root) {
                return Err("managed output directory escaped its private root".to_string());
            }
            scan_directory(root, &canonical, depth.saturating_add(1), state)?;
            continue;
        }
        if !metadata.file_type().is_file() {
            return Err(
                "managed outputs must contain only regular files and directories".to_string(),
            );
        }
        state.total_bytes = state
            .total_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| "managed output byte count overflowed".to_string())?;
        if state.total_bytes > MAX_MANAGED_COMMAND_OUTPUT_WORKSPACE_BYTES {
            return Err(format!(
                "managed output workspace exceeds the {MAX_MANAGED_COMMAND_OUTPUT_WORKSPACE_BYTES} byte safety limit"
            ));
        }
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("managed output file cannot be verified: {error}"))?;
        if !canonical.starts_with(root) {
            return Err("managed output file escaped its private root".to_string());
        }
        let relative = canonical
            .strip_prefix(root)
            .map_err(|_| "managed output file escaped its private root".to_string())?;
        let relative_name = portable_relative_name(relative)?;
        ensure_supported_extension(&relative_name)?;
        state.files.push(ScannedOutput {
            relative_name,
            absolute_path: canonical,
            sha256: hash_regular_file(&path, metadata.len())?,
            size_bytes: metadata.len(),
        });
    }
    Ok(())
}

fn hash_regular_file(path: &Path, expected_size: u64) -> Result<String, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("managed output could not be opened safely: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("managed output metadata is unavailable: {error}"))?;
    if !metadata.file_type().is_file() || metadata.len() != expected_size {
        return Err("managed output changed while it was being scanned".to_string());
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("managed output could not be hashed: {error}"))?;
        if read == 0 {
            break;
        }
        bytes = bytes.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
    }
    if bytes != expected_size {
        return Err("managed output changed while it was being scanned".to_string());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn portable_relative_name(path: &Path) -> Result<String, String> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let value = value
                    .to_str()
                    .ok_or_else(|| "managed output names must be valid UTF-8".to_string())?;
                if value.is_empty() || value.chars().any(char::is_control) {
                    return Err("managed output names contain unsupported characters".to_string());
                }
                components.push(value);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("managed output name is unsafe".to_string())
            }
        }
    }
    if components.is_empty() {
        Err("managed output name is empty".to_string())
    } else {
        Ok(components.join("/"))
    }
}

fn ensure_supported_extension(name: &str) -> Result<(), String> {
    let extension = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    if extension
        .as_deref()
        .is_some_and(|extension| matches!(extension, "png" | "jpg" | "jpeg" | "webp" | "pdf"))
    {
        Ok(())
    } else {
        Err(format!(
            "managed output `{name}` has an unsupported type; expected PNG, JPEG, WebP, or PDF"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::ProcessOutputCaptureMetadata;
    use std::fs;

    fn successful_execution(capture: ManagedCommandOutputCapture) -> AgentCommandExecutionResult {
        AgentCommandExecutionResult {
            outputs: Vec::new(),
            command: "pdf-render".to_string(),
            cwd: "<managed>".to_string(),
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
            cancelled: false,
            duration_ms: 1,
            stdout_truncated: false,
            stderr_truncated: false,
            output_capture: ProcessOutputCaptureMetadata::default(),
            stdout_spool: Default::default(),
            stderr_spool: Default::default(),
            error: None,
            policy_evaluation: None,
            artifact_observation: None,
            input_files: Vec::new(),
            runtime: None,
            managed_outputs: Some(capture),
            authoritative_archive_ref: None,
            history_open: None,
        }
    }

    fn test_capture() -> (tempfile::TempDir, ManagedCommandOutputCapture) {
        let root = tempfile::tempdir().unwrap();
        let lease =
            super::super::ManagedCommandWorkspaceLease::open(root.path().join("managed-run"))
                .unwrap();
        let capture = lease.output_capture(BTreeMap::new());
        (root, capture)
    }

    #[test]
    fn scan_is_deterministic_and_rejects_symlinks_and_unsupported_files() {
        let (_root, capture) = test_capture();
        fs::create_dir(capture.root().join("pages")).unwrap();
        fs::write(capture.root().join("pages/b.pdf"), b"%PDF-1.7\n%%EOF\n").unwrap();
        fs::write(capture.root().join("a.pdf"), b"%PDF-1.7\n%%EOF\n").unwrap();
        let files = scan_output_files(&capture).unwrap();
        assert_eq!(
            files
                .iter()
                .map(|file| file.relative_name.as_str())
                .collect::<Vec<_>>(),
            ["a.pdf", "pages/b.pdf"]
        );

        fs::write(capture.root().join("notes.txt"), b"not publishable").unwrap();
        assert!(scan_output_files(&capture)
            .unwrap_err()
            .contains("unsupported type"));

        #[cfg(unix)]
        {
            fs::remove_file(capture.root().join("notes.txt")).unwrap();
            std::os::unix::fs::symlink(
                capture.root().join("a.pdf"),
                capture.root().join("link.pdf"),
            )
            .unwrap();
            assert!(scan_output_files(&capture)
                .unwrap_err()
                .contains("symbolic links"));
        }
    }

    #[test]
    fn failed_commands_do_not_publish_outputs() {
        let (_root, capture) = test_capture();
        fs::write(capture.root().join("report.pdf"), b"%PDF-1.7\n%%EOF\n").unwrap();
        let mut execution = successful_execution(capture);
        execution.exit_code = Some(1);
        let storage_root = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&storage_root.path().join("storage.sqlite")).unwrap();
        publish_managed_command_outputs(
            &storage,
            &mut execution,
            "conversation-1",
            "run-1",
            "call-1",
        )
        .unwrap();
        assert!(execution.outputs.is_empty());
    }

    #[test]
    fn publication_limits_apply_to_each_changed_batch_not_the_accumulated_run() {
        let (_root, first_capture) = test_capture();
        for index in 0..MAX_MANAGED_COMMAND_OUTPUT_FILES {
            fs::write(
                first_capture.root().join(format!("page-{index:03}.pdf")),
                b"%PDF-1.7\n%%EOF\n",
            )
            .unwrap();
        }
        let baseline = snapshot_managed_command_output_baseline(first_capture.root()).unwrap();
        let second_capture = first_capture.workspace().output_capture(baseline);
        fs::write(
            second_capture.root().join("page-032.pdf"),
            b"%PDF-1.7\nsecond\n%%EOF\n",
        )
        .unwrap();
        fs::write(
            second_capture.root().join("page-033.pdf"),
            b"%PDF-1.7\nthird\n%%EOF\n",
        )
        .unwrap();

        let files = scan_output_files(&second_capture).unwrap();
        assert_eq!(
            files
                .iter()
                .map(|file| file.relative_name.as_str())
                .collect::<Vec<_>>(),
            ["page-032.pdf", "page-033.pdf"]
        );
    }
}
