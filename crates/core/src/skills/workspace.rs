//! Workspace-backed Skill source access.
//!
//! Discovery and resolution deliberately share this module. It owns layout
//! validation, exact-case file lookup, bounded reads, and platform-specific
//! no-follow/identity checks so the two phases cannot drift apart.

use super::model::{SkillDiagnosticCode, SkillDiagnosticSeverity, SkillDiscoveryError};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

#[cfg(windows)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
#[cfg(windows)]
use std::os::windows::{
    ffi::{OsStrExt, OsStringExt},
    fs::OpenOptionsExt,
    io::AsRawHandle,
};
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, GetFinalPathNameByHandleW,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
        FILE_NAME_NORMALIZED, SECURITY_IDENTIFICATION, VOLUME_NAME_DOS,
    },
};

pub(super) const SKILLS_DIRECTORY: &str = "skills";
pub(super) const AGENTS_DIRECTORY: &str = ".agents";
pub(super) const SKILL_FILE_NAME: &str = "SKILL.md";
pub(super) const MAX_SKILL_FILE_BYTES: usize = 256 * 1024;
pub(super) const MAX_SKILL_ROOT_ENTRIES: usize = 2_000;
#[allow(dead_code)]
pub(super) const MAX_SKILL_DIRECTORY_ENTRIES: usize = 1_024;
pub(super) const MAX_SKILL_SCAN_ENTRIES: usize = 10_000;
pub(super) const MAX_SKILL_CATALOG_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct WorkspaceSkillRoots {
    pub workspace_root: PathBuf,
    pub skills_root: PathBuf,
}

#[derive(Debug)]
pub(super) enum WorkspaceSkillsRoot {
    Missing,
    Ready(WorkspaceSkillRoots),
}

#[derive(Debug)]
pub(super) enum WorkspaceRootError {
    Discovery(SkillDiscoveryError),
    Invalid(WorkspaceSourceIssue),
}

#[allow(dead_code)]
#[derive(Debug)]
pub(super) struct WorkspaceSourceIssue {
    pub code: SkillDiagnosticCode,
    pub severity: SkillDiagnosticSeverity,
    pub path: PathBuf,
    pub message: String,
}

#[allow(dead_code)]
impl WorkspaceSourceIssue {
    fn error(
        path: impl Into<PathBuf>,
        code: SkillDiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: SkillDiagnosticSeverity::Error,
            path: path.into(),
            message: message.into(),
        }
    }

    fn warning(
        path: impl Into<PathBuf>,
        code: SkillDiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: SkillDiagnosticSeverity::Warning,
            path: path.into(),
            message: message.into(),
        }
    }

    pub fn truncates_catalog(&self) -> bool {
        matches!(
            self.code,
            SkillDiagnosticCode::ScanBudgetExceeded | SkillDiagnosticCode::CatalogTooLarge
        )
    }
}

pub(super) fn resolve_workspace_skills_root(
    workspace_root: &Path,
) -> Result<WorkspaceSkillsRoot, WorkspaceRootError> {
    let workspace_root = workspace_root.canonicalize().map_err(|error| {
        WorkspaceRootError::Discovery(SkillDiscoveryError::WorkspaceUnavailable {
            path: workspace_root.to_path_buf(),
            reason: error.to_string(),
        })
    })?;
    if !workspace_root.is_dir() {
        return Err(WorkspaceRootError::Discovery(
            SkillDiscoveryError::WorkspaceNotDirectory {
                path: workspace_root,
            },
        ));
    }

    let agents_root = workspace_root.join(AGENTS_DIRECTORY);
    let Some(agents_metadata) = metadata_if_present(&agents_root).map_err(|error| {
        WorkspaceRootError::Discovery(SkillDiscoveryError::WorkspaceUnavailable {
            path: agents_root.clone(),
            reason: error.to_string(),
        })
    })?
    else {
        return Ok(WorkspaceSkillsRoot::Missing);
    };
    if agents_metadata.file_type().is_symlink() {
        return Err(WorkspaceRootError::Invalid(WorkspaceSourceIssue::error(
            &agents_root,
            SkillDiagnosticCode::SymlinkNotAllowed,
            "Workspace Skill access does not follow a symlinked .agents directory.",
        )));
    }
    if !agents_metadata.is_dir() {
        return Err(WorkspaceRootError::Invalid(WorkspaceSourceIssue::error(
            &agents_root,
            SkillDiagnosticCode::InvalidRoot,
            "The workspace .agents path is not a directory.",
        )));
    }
    let canonical_agents_root = agents_root.canonicalize().map_err(|error| {
        WorkspaceRootError::Invalid(WorkspaceSourceIssue::error(
            &agents_root,
            SkillDiagnosticCode::InvalidRoot,
            format!("Cannot resolve the workspace .agents directory: {error}"),
        ))
    })?;
    if !canonical_agents_root.starts_with(&workspace_root) {
        return Err(WorkspaceRootError::Invalid(WorkspaceSourceIssue::error(
            &agents_root,
            SkillDiagnosticCode::RootEscapesWorkspace,
            "The workspace .agents directory resolves outside the workspace.",
        )));
    }
    verify_plain_directory(
        &agents_root,
        &canonical_agents_root,
        "The workspace .agents directory changed while it was inspected.",
    )
    .map_err(WorkspaceRootError::Invalid)?;

    let skills_root = agents_root.join(SKILLS_DIRECTORY);
    let Some(skills_metadata) = metadata_if_present(&skills_root).map_err(|error| {
        WorkspaceRootError::Discovery(SkillDiscoveryError::WorkspaceUnavailable {
            path: skills_root.clone(),
            reason: error.to_string(),
        })
    })?
    else {
        return Ok(WorkspaceSkillsRoot::Missing);
    };
    if skills_metadata.file_type().is_symlink() {
        return Err(WorkspaceRootError::Invalid(WorkspaceSourceIssue::error(
            &skills_root,
            SkillDiagnosticCode::SymlinkNotAllowed,
            "Workspace Skill access does not follow a symlinked skills directory.",
        )));
    }
    if !skills_metadata.is_dir() {
        return Err(WorkspaceRootError::Invalid(WorkspaceSourceIssue::error(
            &skills_root,
            SkillDiagnosticCode::InvalidRoot,
            "The workspace Skill root is not a directory.",
        )));
    }

    let canonical_skills_root = skills_root.canonicalize().map_err(|error| {
        WorkspaceRootError::Invalid(WorkspaceSourceIssue::error(
            &skills_root,
            SkillDiagnosticCode::InvalidRoot,
            format!("Cannot resolve the workspace Skill root: {error}"),
        ))
    })?;
    if !canonical_skills_root.starts_with(&canonical_agents_root)
        || !canonical_skills_root.starts_with(&workspace_root)
    {
        return Err(WorkspaceRootError::Invalid(WorkspaceSourceIssue::error(
            &skills_root,
            SkillDiagnosticCode::RootEscapesWorkspace,
            "The workspace Skill root resolves outside the workspace.",
        )));
    }
    verify_plain_directory(
        &skills_root,
        &canonical_skills_root,
        "The workspace Skill root changed while it was inspected.",
    )
    .map_err(WorkspaceRootError::Invalid)?;

    Ok(WorkspaceSkillsRoot::Ready(WorkspaceSkillRoots {
        workspace_root,
        skills_root: canonical_skills_root,
    }))
}

pub(super) fn verify_plain_directory(
    lexical_path: &Path,
    expected_canonical_path: &Path,
    changed_message: &str,
) -> Result<(), WorkspaceSourceIssue> {
    let metadata = fs::symlink_metadata(lexical_path).map_err(|error| {
        WorkspaceSourceIssue::error(
            lexical_path,
            SkillDiagnosticCode::PathChangedDuringRead,
            format!("{changed_message} Cannot inspect it again: {error}"),
        )
    })?;
    if is_symlink_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(WorkspaceSourceIssue::error(
            lexical_path,
            SkillDiagnosticCode::PathChangedDuringRead,
            changed_message,
        ));
    }
    let current_canonical_path = lexical_path.canonicalize().map_err(|error| {
        WorkspaceSourceIssue::error(
            lexical_path,
            SkillDiagnosticCode::PathChangedDuringRead,
            format!("{changed_message} Cannot resolve it again: {error}"),
        )
    })?;
    if current_canonical_path != expected_canonical_path {
        return Err(WorkspaceSourceIssue::error(
            lexical_path,
            SkillDiagnosticCode::PathChangedDuringRead,
            changed_message,
        ));
    }
    Ok(())
}

pub(super) fn is_symlink_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        return metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    #[cfg(not(windows))]
    false
}

#[allow(dead_code)]
#[derive(Debug)]
pub(super) struct LoadedWorkspaceSkill {
    pub directory_name: String,
    pub canonical_path: PathBuf,
    pub relative_path: String,
    pub bytes: Vec<u8>,
}

#[allow(dead_code)]
pub(super) fn load_workspace_skill(
    roots: &WorkspaceSkillRoots,
    skill_directory: &Path,
    scan_budget: &mut ScanBudget,
    byte_budget: &mut ByteBudget,
) -> Result<LoadedWorkspaceSkill, WorkspaceSourceIssue> {
    let directory_name = skill_directory
        .file_name()
        .and_then(OsStr::to_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            WorkspaceSourceIssue::error(
                skill_directory,
                SkillDiagnosticCode::UnsupportedPathEncoding,
                "Skill directory names must be valid UTF-8.",
            )
        })?
        .to_owned();
    validate_skill_directory_name(&directory_name).map_err(|reason| {
        WorkspaceSourceIssue::error(
            skill_directory,
            SkillDiagnosticCode::InvalidDirectoryName,
            reason,
        )
    })?;

    let directory_metadata = fs::symlink_metadata(skill_directory).map_err(|error| {
        WorkspaceSourceIssue::error(
            skill_directory,
            SkillDiagnosticCode::UnreadableEntry,
            format!("Cannot inspect Skill directory: {error}"),
        )
    })?;
    if directory_metadata.file_type().is_symlink() {
        return Err(WorkspaceSourceIssue::error(
            skill_directory,
            SkillDiagnosticCode::SymlinkNotAllowed,
            "Workspace Skill access does not follow symlinked Skill directories.",
        ));
    }
    if !directory_metadata.is_dir() {
        return Err(WorkspaceSourceIssue::error(
            skill_directory,
            SkillDiagnosticCode::MissingSkillFile,
            "The selected Skill path is not a directory.",
        ));
    }
    let canonical_skill_directory = skill_directory.canonicalize().map_err(|error| {
        WorkspaceSourceIssue::error(
            skill_directory,
            SkillDiagnosticCode::UnreadableEntry,
            format!("Cannot resolve Skill directory: {error}"),
        )
    })?;
    if !canonical_skill_directory.starts_with(&roots.skills_root) {
        return Err(WorkspaceSourceIssue::error(
            skill_directory,
            SkillDiagnosticCode::RootEscapesWorkspace,
            "Skill directory resolves outside the workspace Skill root.",
        ));
    }
    verify_plain_directory(
        skill_directory,
        &canonical_skill_directory,
        "The Skill directory changed while it was inspected.",
    )?;

    let skill_file = match find_exact_skill_file(skill_directory, scan_budget).map_err(|error| {
        WorkspaceSourceIssue::error(
            skill_directory,
            SkillDiagnosticCode::UnreadableEntry,
            format!("Cannot inspect Skill directory: {error}"),
        )
    })? {
        ExactSkillFile::Found(path) => path,
        ExactSkillFile::Missing => {
            return Err(WorkspaceSourceIssue::error(
                skill_directory,
                SkillDiagnosticCode::MissingSkillFile,
                "Skill directory does not contain an exact-case SKILL.md file.",
            ));
        }
        ExactSkillFile::TooManyEntries => {
            return Err(WorkspaceSourceIssue::error(
                skill_directory,
                SkillDiagnosticCode::TooManyEntries,
                format!(
                    "Skill directory contains more than {MAX_SKILL_DIRECTORY_ENTRIES} entries."
                ),
            ));
        }
        ExactSkillFile::ScanBudgetExceeded => {
            return Err(WorkspaceSourceIssue::warning(
                &roots.skills_root,
                SkillDiagnosticCode::ScanBudgetExceeded,
                format!(
                    "Workspace Skill access exceeded its {MAX_SKILL_SCAN_ENTRIES}-entry scan budget."
                ),
            ));
        }
    };

    let file_metadata = fs::symlink_metadata(&skill_file).map_err(|error| {
        WorkspaceSourceIssue::error(
            &skill_file,
            SkillDiagnosticCode::UnreadableEntry,
            format!("Cannot inspect SKILL.md: {error}"),
        )
    })?;
    if file_metadata.file_type().is_symlink() {
        return Err(WorkspaceSourceIssue::error(
            &skill_file,
            SkillDiagnosticCode::SymlinkNotAllowed,
            "Workspace Skill access does not follow a symlinked SKILL.md.",
        ));
    }
    if !file_metadata.is_file() {
        return Err(WorkspaceSourceIssue::error(
            &skill_file,
            SkillDiagnosticCode::MissingSkillFile,
            "SKILL.md is not a regular file.",
        ));
    }

    let canonical_skill_file = skill_file.canonicalize().map_err(|error| {
        WorkspaceSourceIssue::error(
            &skill_file,
            SkillDiagnosticCode::UnreadableEntry,
            format!("Cannot resolve SKILL.md: {error}"),
        )
    })?;
    if !canonical_skill_file.starts_with(&canonical_skill_directory)
        || !canonical_skill_file.starts_with(&roots.workspace_root)
    {
        return Err(WorkspaceSourceIssue::error(
            &skill_file,
            SkillDiagnosticCode::RootEscapesWorkspace,
            "SKILL.md resolves outside its Skill directory.",
        ));
    }

    let bytes = read_bounded_verified(
        &skill_file,
        &canonical_skill_file,
        &file_metadata,
        &canonical_skill_directory,
        &roots.workspace_root,
        MAX_SKILL_FILE_BYTES,
        byte_budget,
    )
    .map_err(|error| match error {
        BoundedReadError::CatalogBudgetExceeded => WorkspaceSourceIssue::warning(
            &roots.skills_root,
            SkillDiagnosticCode::CatalogTooLarge,
            format!(
                "Workspace Skill catalog exceeds the {MAX_SKILL_CATALOG_BYTES}-byte scan budget."
            ),
        ),
        BoundedReadError::TooLarge => WorkspaceSourceIssue::error(
            &canonical_skill_file,
            SkillDiagnosticCode::SkillFileTooLarge,
            format!("SKILL.md exceeds {MAX_SKILL_FILE_BYTES} bytes."),
        ),
        BoundedReadError::Io(error) => WorkspaceSourceIssue::error(
            &canonical_skill_file,
            SkillDiagnosticCode::UnreadableEntry,
            format!("Cannot read SKILL.md: {error}"),
        ),
        BoundedReadError::PathChanged(reason) => WorkspaceSourceIssue::error(
            &skill_file,
            SkillDiagnosticCode::PathChangedDuringRead,
            format!("SKILL.md changed or became unsafe while it was inspected: {reason}"),
        ),
    })?;

    // Fail closed if an intermediate directory changed after the file handle
    // was verified. The returned bytes are safe, but they no longer represent
    // the same source layout selected by discovery.
    verify_plain_directory(
        skill_directory,
        &canonical_skill_directory,
        "The Skill directory changed while SKILL.md was read.",
    )?;

    Ok(LoadedWorkspaceSkill {
        directory_name,
        relative_path: relative_display(&roots.workspace_root, &canonical_skill_file),
        canonical_path: canonical_skill_file,
        bytes,
    })
}

#[derive(Debug)]
pub(super) struct ScanBudget {
    pub(super) remaining_entries: usize,
}

impl ScanBudget {
    pub fn new(max_entries: usize) -> Self {
        Self {
            remaining_entries: max_entries,
        }
    }

    pub fn consume_entry(&mut self) -> bool {
        if self.remaining_entries == 0 {
            return false;
        }
        self.remaining_entries -= 1;
        true
    }
}

#[derive(Debug)]
pub(super) struct ByteBudget {
    pub(super) remaining_bytes: usize,
}

impl ByteBudget {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            remaining_bytes: max_bytes,
        }
    }

    fn consume(&mut self, bytes: usize) {
        self.remaining_bytes = self.remaining_bytes.saturating_sub(bytes);
    }
}

#[allow(dead_code)]
pub(super) enum ExactSkillFile {
    Found(PathBuf),
    Missing,
    TooManyEntries,
    ScanBudgetExceeded,
}

#[allow(dead_code)]
pub(super) fn find_exact_skill_file(
    skill_directory: &Path,
    scan_budget: &mut ScanBudget,
) -> io::Result<ExactSkillFile> {
    let entries = fs::read_dir(skill_directory)?;
    let mut exact_skill_file = None;
    let mut observed_entries = 0usize;
    for entry in entries {
        if !scan_budget.consume_entry() {
            return Ok(ExactSkillFile::ScanBudgetExceeded);
        }
        observed_entries = observed_entries.saturating_add(1);
        if observed_entries > MAX_SKILL_DIRECTORY_ENTRIES {
            return Ok(ExactSkillFile::TooManyEntries);
        }
        let entry = entry?;
        if entry.file_name() == OsStr::new(SKILL_FILE_NAME) {
            exact_skill_file = Some(entry.path());
        }
    }
    Ok(exact_skill_file
        .map(ExactSkillFile::Found)
        .unwrap_or(ExactSkillFile::Missing))
}

pub(super) enum BoundedReadError {
    Io(io::Error),
    TooLarge,
    CatalogBudgetExceeded,
    PathChanged(String),
}

pub(super) fn read_bounded_verified(
    lexical_path: &Path,
    expected_canonical_path: &Path,
    expected_metadata: &fs::Metadata,
    canonical_skill_directory: &Path,
    canonical_workspace_root: &Path,
    max_bytes: usize,
    byte_budget: &mut ByteBudget,
) -> Result<Vec<u8>, BoundedReadError> {
    let file = open_skill_file(lexical_path).map_err(|error| {
        BoundedReadError::PathChanged(format!("cannot safely open the checked path: {error}"))
    })?;
    let opened_metadata = file.metadata().map_err(|error| {
        BoundedReadError::PathChanged(format!("cannot inspect the opened file: {error}"))
    })?;
    if !opened_metadata.is_file() {
        return Err(BoundedReadError::PathChanged(
            "the opened object is not a regular file".to_string(),
        ));
    }
    verify_opened_file_identity(
        &file,
        expected_metadata,
        &opened_metadata,
        expected_canonical_path,
    )
    .map_err(BoundedReadError::PathChanged)?;

    let resolved_path = lexical_path.canonicalize().map_err(|error| {
        BoundedReadError::PathChanged(format!("cannot re-resolve the opened path: {error}"))
    })?;
    if resolved_path != expected_canonical_path
        || !resolved_path.starts_with(canonical_skill_directory)
        || !resolved_path.starts_with(canonical_workspace_root)
    {
        return Err(BoundedReadError::PathChanged(
            "the opened path no longer resolves to the checked workspace file".to_string(),
        ));
    }

    read_open_file_bounded(file, max_bytes, byte_budget)
}

fn open_skill_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    #[cfg(windows)]
    options
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .security_qos_flags(SECURITY_IDENTIFICATION);
    options.open(path)
}

#[cfg(unix)]
pub(super) fn verify_opened_file_identity(
    _file: &File,
    expected: &fs::Metadata,
    opened: &fs::Metadata,
    _expected_canonical_path: &Path,
) -> Result<(), String> {
    if expected.dev() == opened.dev() && expected.ino() == opened.ino() {
        Ok(())
    } else {
        Err("the opened file is not the file that was checked".to_string())
    }
}

#[cfg(windows)]
pub(super) fn verify_opened_file_identity(
    file: &File,
    _expected: &fs::Metadata,
    opened: &fs::Metadata,
    expected_canonical_path: &Path,
) -> Result<(), String> {
    let opened_identity = windows_file_identity(file)
        .map_err(|error| format!("cannot identify the opened file: {error}"))?;
    let opened_path = windows_final_path(file)
        .map_err(|error| format!("cannot resolve the opened file handle: {error}"))?;
    if !windows_paths_equivalent(&opened_path, expected_canonical_path) {
        return Err("the opened handle resolves outside the checked workspace path".to_string());
    }

    let expected_file = open_windows_identity_path(expected_canonical_path, opened.is_dir())
        .map_err(|error| format!("cannot reopen the checked canonical path: {error}"))?;
    let expected_identity = windows_file_identity(&expected_file)
        .map_err(|error| format!("cannot identify the checked canonical file: {error}"))?;
    if opened_identity != expected_identity {
        return Err("the opened file is not the file that was checked".to_string());
    }
    Ok(())
}

#[cfg(windows)]
fn open_windows_identity_path(path: &Path, is_directory: bool) -> io::Result<File> {
    let mut flags = FILE_FLAG_OPEN_REPARSE_POINT;
    if is_directory {
        // Windows requires BACKUP_SEMANTICS when opening a directory handle.
        flags |= FILE_FLAG_BACKUP_SEMANTICS;
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(flags)
        .security_qos_flags(SECURITY_IDENTIFICATION);
    options.open(path)
}

#[cfg(windows)]
#[derive(Debug, PartialEq, Eq)]
struct WindowsFileIdentity {
    volume_serial_number: u64,
    file_id: [u8; 16],
}

#[cfg(windows)]
fn windows_file_identity(file: &File) -> io::Result<WindowsFileIdentity> {
    let mut information = FILE_ID_INFO::default();
    // SAFETY: `file` keeps the HANDLE valid for the call, and `information` is a writable
    // FILE_ID_INFO buffer whose exact byte size is supplied to Windows.
    let result = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as HANDLE,
            FileIdInfo,
            (&raw mut information).cast(),
            u32::try_from(std::mem::size_of::<FILE_ID_INFO>()).unwrap_or(u32::MAX),
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(WindowsFileIdentity {
        volume_serial_number: information.VolumeSerialNumber,
        file_id: information.FileId.Identifier,
    })
}

#[cfg(windows)]
fn windows_final_path(file: &File) -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 512];
    loop {
        let capacity = u32::try_from(buffer.len())
            .map_err(|_| io::Error::other("Windows path buffer exceeds u32"))?;
        // SAFETY: `file` keeps the HANDLE valid and `buffer` exposes `capacity` writable u16s.
        // Windows reports either the number written or the larger capacity required.
        let written = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle() as HANDLE,
                buffer.as_mut_ptr(),
                capacity,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        };
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        let written = usize::try_from(written)
            .map_err(|_| io::Error::other("Windows path length exceeds usize"))?;
        if written < buffer.len() {
            buffer.truncate(written);
            return Ok(PathBuf::from(OsString::from_wide(&buffer)));
        }
        buffer.resize(written.saturating_add(1), 0);
    }
}

#[cfg(windows)]
fn windows_paths_equivalent(left: &Path, right: &Path) -> bool {
    windows_path_key(left) == windows_path_key(right)
}

#[cfg(windows)]
fn windows_path_key(path: &Path) -> Vec<u16> {
    normalize_windows_path_units(path.as_os_str().encode_wide().collect())
}

#[cfg(any(windows, test))]
pub(super) fn normalize_windows_path_units(mut units: Vec<u16>) -> Vec<u16> {
    const BACKSLASH: u16 = b'\\' as u16;
    let verbatim_prefix = [BACKSLASH, BACKSLASH, b'?' as u16, BACKSLASH];
    let verbatim_unc_prefix = [
        BACKSLASH,
        BACKSLASH,
        b'?' as u16,
        BACKSLASH,
        b'U' as u16,
        b'N' as u16,
        b'C' as u16,
        BACKSLASH,
    ];
    if units.starts_with(&verbatim_unc_prefix) {
        units.splice(..verbatim_unc_prefix.len(), [BACKSLASH, BACKSLASH]);
    } else if units.starts_with(&verbatim_prefix) {
        units.drain(..verbatim_prefix.len());
    }
    for unit in &mut units {
        if *unit == b'/' as u16 {
            *unit = BACKSLASH;
        } else if (b'A' as u16..=b'Z' as u16).contains(unit) {
            *unit += u16::from(b'a' - b'A');
        }
    }
    units
}

#[cfg(not(any(unix, windows)))]
pub(super) fn verify_opened_file_identity(
    _file: &File,
    _expected: &fs::Metadata,
    _opened: &fs::Metadata,
    _expected_canonical_path: &Path,
) -> Result<(), String> {
    Err("secure Skill file identity checks are unavailable on this platform".to_string())
}

pub(super) fn read_open_file_bounded(
    file: File,
    max_bytes: usize,
    byte_budget: &mut ByteBudget,
) -> Result<Vec<u8>, BoundedReadError> {
    let remaining_catalog_bytes = byte_budget.remaining_bytes;
    if remaining_catalog_bytes == 0 {
        return Err(BoundedReadError::CatalogBudgetExceeded);
    }
    let read_limit = max_bytes
        .saturating_add(1)
        .min(remaining_catalog_bytes.saturating_add(1));
    let limit = u64::try_from(read_limit).unwrap_or(u64::MAX);
    let mut bytes = Vec::with_capacity(max_bytes.min(16 * 1024));
    let read_result = file.take(limit).read_to_end(&mut bytes);
    byte_budget.consume(bytes.len());
    if bytes.len() > remaining_catalog_bytes {
        return Err(BoundedReadError::CatalogBudgetExceeded);
    }
    read_result.map_err(BoundedReadError::Io)?;
    if bytes.len() > max_bytes {
        Err(BoundedReadError::TooLarge)
    } else {
        Ok(bytes)
    }
}

pub(super) fn metadata_if_present(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn validate_skill_directory_name(directory_name: &str) -> Result<(), &'static str> {
    if directory_name.is_empty() {
        return Err("Skill directory names must not be empty.");
    }
    if directory_name.starts_with('.') {
        return Err("Hidden Skill directories are not discoverable.");
    }
    if directory_name.contains(['/', '\\', '\0']) || directory_name.chars().any(char::is_control) {
        return Err(
            "Skill directory names cannot contain path separators, NUL, or control characters.",
        );
    }
    Ok(())
}

pub(super) fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn percent_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len());
    for byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            use std::fmt::Write;
            write!(&mut encoded, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn plain_directory_recheck_rejects_a_symlink_replacement() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        let lexical = root.path().join("skill");
        fs::create_dir(&lexical).unwrap();
        let expected = lexical.canonicalize().unwrap();
        let moved = root.path().join("moved");
        fs::rename(&lexical, &moved).unwrap();
        symlink(&moved, &lexical).unwrap();

        let issue = verify_plain_directory(&lexical, &expected, "changed").unwrap_err();

        assert_eq!(issue.code, SkillDiagnosticCode::PathChangedDuringRead);
    }

    #[cfg(windows)]
    #[test]
    fn identity_verification_can_reopen_a_directory_handle() {
        let root = tempdir().unwrap();
        let canonical = root.path().canonicalize().unwrap();
        let expected = fs::symlink_metadata(root.path()).unwrap();
        let directory = open_windows_identity_path(root.path(), true).unwrap();
        let opened = directory.metadata().unwrap();

        verify_opened_file_identity(&directory, &expected, &opened, &canonical).unwrap();
    }
}
