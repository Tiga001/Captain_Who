use super::model::{
    SkillCatalog, SkillDescriptor, SkillDiagnostic, SkillDiagnosticCode, SkillDiagnosticSeverity,
    SkillDiscoveryError, SkillScope,
};
use super::parser::parse_skill_metadata;
use crate::content_revision;
use std::collections::BTreeMap;
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
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_NAME_NORMALIZED, SECURITY_IDENTIFICATION,
        VOLUME_NAME_DOS,
    },
};

const SKILLS_DIRECTORY: &str = "skills";
const AGENTS_DIRECTORY: &str = ".agents";
const SKILL_FILE_NAME: &str = "SKILL.md";
const MAX_SKILL_FILE_BYTES: usize = 256 * 1024;
const MAX_SKILL_ROOT_ENTRIES: usize = 2_000;
const MAX_SKILL_DIRECTORY_ENTRIES: usize = 1_024;
const MAX_SKILL_SCAN_ENTRIES: usize = 10_000;
const MAX_SKILL_CATALOG_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct SkillsService;

impl SkillsService {
    pub fn new() -> Self {
        Self
    }

    pub fn list_workspace(
        &self,
        workspace_id: &str,
        workspace_root: &Path,
    ) -> Result<SkillCatalog, SkillDiscoveryError> {
        let workspace_root = workspace_root.canonicalize().map_err(|error| {
            SkillDiscoveryError::WorkspaceUnavailable {
                path: workspace_root.to_path_buf(),
                reason: error.to_string(),
            }
        })?;
        if !workspace_root.is_dir() {
            return Err(SkillDiscoveryError::WorkspaceNotDirectory {
                path: workspace_root,
            });
        }

        let agents_root = workspace_root.join(AGENTS_DIRECTORY);
        let Some(agents_metadata) = metadata_if_present(&agents_root).map_err(|error| {
            SkillDiscoveryError::WorkspaceUnavailable {
                path: agents_root.clone(),
                reason: error.to_string(),
            }
        })?
        else {
            return Ok(finalize_catalog(Vec::new(), Vec::new(), false));
        };
        if agents_metadata.file_type().is_symlink() {
            return Ok(catalog_with_root_diagnostic(
                &agents_root,
                SkillDiagnosticCode::SymlinkNotAllowed,
                "Workspace Skill discovery does not follow a symlinked .agents directory.",
            ));
        }
        if !agents_metadata.is_dir() {
            return Ok(catalog_with_root_diagnostic(
                &agents_root,
                SkillDiagnosticCode::InvalidRoot,
                "The workspace .agents path is not a directory.",
            ));
        }

        let skills_root = agents_root.join(SKILLS_DIRECTORY);
        let Some(skills_metadata) = metadata_if_present(&skills_root).map_err(|error| {
            SkillDiscoveryError::WorkspaceUnavailable {
                path: skills_root.clone(),
                reason: error.to_string(),
            }
        })?
        else {
            return Ok(finalize_catalog(Vec::new(), Vec::new(), false));
        };
        if skills_metadata.file_type().is_symlink() {
            return Ok(catalog_with_root_diagnostic(
                &skills_root,
                SkillDiagnosticCode::SymlinkNotAllowed,
                "Workspace Skill discovery does not follow a symlinked skills directory.",
            ));
        }
        if !skills_metadata.is_dir() {
            return Ok(catalog_with_root_diagnostic(
                &skills_root,
                SkillDiagnosticCode::InvalidRoot,
                "The workspace Skill root is not a directory.",
            ));
        }

        let canonical_skills_root = match skills_root.canonicalize() {
            Ok(root) if root.starts_with(&workspace_root) => root,
            Ok(_) => {
                return Ok(catalog_with_root_diagnostic(
                    &skills_root,
                    SkillDiagnosticCode::RootEscapesWorkspace,
                    "The workspace Skill root resolves outside the workspace.",
                ));
            }
            Err(error) => {
                return Ok(catalog_with_root_diagnostic(
                    &skills_root,
                    SkillDiagnosticCode::InvalidRoot,
                    format!("Cannot resolve the workspace Skill root: {error}"),
                ));
            }
        };

        let mut scan_budget = ScanBudget::new(MAX_SKILL_SCAN_ENTRIES);
        let (candidates, mut diagnostics, root_truncated) =
            collect_skill_directories(&canonical_skills_root, &mut scan_budget);
        if root_truncated {
            return Ok(finalize_catalog(Vec::new(), diagnostics, true));
        }

        let mut skills = Vec::new();
        let mut byte_budget = ByteBudget::new(MAX_SKILL_CATALOG_BYTES);
        let mut catalog_truncated = false;
        for skill_directory in candidates {
            let directory_name = match skill_directory.file_name().and_then(OsStr::to_str) {
                Some(name) if !name.is_empty() => name,
                _ => {
                    diagnostics.push(diagnostic(
                        &skill_directory,
                        SkillDiagnosticCode::UnsupportedPathEncoding,
                        SkillDiagnosticSeverity::Error,
                        "Skill directory names must be valid UTF-8.",
                    ));
                    continue;
                }
            };

            let skill_file = match find_exact_skill_file(&skill_directory, &mut scan_budget) {
                Ok(ExactSkillFile::Found(path)) => path,
                Ok(ExactSkillFile::Missing) => {
                    diagnostics.push(diagnostic(
                        &skill_directory,
                        SkillDiagnosticCode::MissingSkillFile,
                        SkillDiagnosticSeverity::Error,
                        "Skill directory does not contain an exact-case SKILL.md file.",
                    ));
                    continue;
                }
                Ok(ExactSkillFile::TooManyEntries) => {
                    diagnostics.push(diagnostic(
                        &skill_directory,
                        SkillDiagnosticCode::TooManyEntries,
                        SkillDiagnosticSeverity::Error,
                        format!(
                            "Skill directory contains more than {MAX_SKILL_DIRECTORY_ENTRIES} entries."
                        ),
                    ));
                    continue;
                }
                Ok(ExactSkillFile::ScanBudgetExceeded) => {
                    diagnostics.push(diagnostic(
                        &canonical_skills_root,
                        SkillDiagnosticCode::ScanBudgetExceeded,
                        SkillDiagnosticSeverity::Warning,
                        format!(
                            "Workspace Skill discovery exceeded its {MAX_SKILL_SCAN_ENTRIES}-entry scan budget."
                        ),
                    ));
                    catalog_truncated = true;
                    break;
                }
                Err(error) => {
                    diagnostics.push(diagnostic(
                        &skill_directory,
                        SkillDiagnosticCode::UnreadableEntry,
                        SkillDiagnosticSeverity::Error,
                        format!("Cannot inspect Skill directory: {error}"),
                    ));
                    continue;
                }
            };

            let file_metadata = match fs::symlink_metadata(&skill_file) {
                Ok(metadata) => metadata,
                Err(error) => {
                    diagnostics.push(diagnostic(
                        &skill_file,
                        SkillDiagnosticCode::UnreadableEntry,
                        SkillDiagnosticSeverity::Error,
                        format!("Cannot inspect SKILL.md: {error}"),
                    ));
                    continue;
                }
            };
            if file_metadata.file_type().is_symlink() {
                diagnostics.push(diagnostic(
                    &skill_file,
                    SkillDiagnosticCode::SymlinkNotAllowed,
                    SkillDiagnosticSeverity::Error,
                    "Workspace Skill discovery does not follow a symlinked SKILL.md.",
                ));
                continue;
            }
            if !file_metadata.is_file() {
                diagnostics.push(diagnostic(
                    &skill_file,
                    SkillDiagnosticCode::MissingSkillFile,
                    SkillDiagnosticSeverity::Error,
                    "SKILL.md is not a regular file.",
                ));
                continue;
            }

            let canonical_skill_directory = match skill_directory.canonicalize() {
                Ok(path) if path.starts_with(&canonical_skills_root) => path,
                Ok(_) => {
                    diagnostics.push(diagnostic(
                        &skill_directory,
                        SkillDiagnosticCode::RootEscapesWorkspace,
                        SkillDiagnosticSeverity::Error,
                        "Skill directory resolves outside the workspace Skill root.",
                    ));
                    continue;
                }
                Err(error) => {
                    diagnostics.push(diagnostic(
                        &skill_directory,
                        SkillDiagnosticCode::UnreadableEntry,
                        SkillDiagnosticSeverity::Error,
                        format!("Cannot resolve Skill directory: {error}"),
                    ));
                    continue;
                }
            };
            let canonical_skill_file = match skill_file.canonicalize() {
                Ok(path)
                    if path.starts_with(&canonical_skill_directory)
                        && path.starts_with(&workspace_root) =>
                {
                    path
                }
                Ok(_) => {
                    diagnostics.push(diagnostic(
                        &skill_file,
                        SkillDiagnosticCode::RootEscapesWorkspace,
                        SkillDiagnosticSeverity::Error,
                        "SKILL.md resolves outside its Skill directory.",
                    ));
                    continue;
                }
                Err(error) => {
                    diagnostics.push(diagnostic(
                        &skill_file,
                        SkillDiagnosticCode::UnreadableEntry,
                        SkillDiagnosticSeverity::Error,
                        format!("Cannot resolve SKILL.md: {error}"),
                    ));
                    continue;
                }
            };

            let bytes = match read_bounded_verified(
                &skill_file,
                &canonical_skill_file,
                &file_metadata,
                &canonical_skill_directory,
                &workspace_root,
                MAX_SKILL_FILE_BYTES,
                &mut byte_budget,
            ) {
                Ok(bytes) => bytes,
                Err(BoundedReadError::CatalogBudgetExceeded) => {
                    diagnostics.push(diagnostic(
                        &canonical_skills_root,
                        SkillDiagnosticCode::CatalogTooLarge,
                        SkillDiagnosticSeverity::Warning,
                        format!(
                            "Workspace Skill catalog exceeds the {MAX_SKILL_CATALOG_BYTES}-byte scan budget."
                        ),
                    ));
                    catalog_truncated = true;
                    break;
                }
                Err(BoundedReadError::TooLarge) => {
                    diagnostics.push(diagnostic(
                        &canonical_skill_file,
                        SkillDiagnosticCode::SkillFileTooLarge,
                        SkillDiagnosticSeverity::Error,
                        format!("SKILL.md exceeds {MAX_SKILL_FILE_BYTES} bytes."),
                    ));
                    continue;
                }
                Err(BoundedReadError::Io(error)) => {
                    diagnostics.push(diagnostic(
                        &canonical_skill_file,
                        SkillDiagnosticCode::UnreadableEntry,
                        SkillDiagnosticSeverity::Error,
                        format!("Cannot read SKILL.md: {error}"),
                    ));
                    continue;
                }
                Err(BoundedReadError::PathChanged(reason)) => {
                    diagnostics.push(diagnostic(
                        &skill_file,
                        SkillDiagnosticCode::PathChangedDuringRead,
                        SkillDiagnosticSeverity::Error,
                        format!(
                            "SKILL.md changed or became unsafe while it was inspected: {reason}"
                        ),
                    ));
                    continue;
                }
            };

            if bytes.contains(&0) {
                diagnostics.push(diagnostic(
                    &canonical_skill_file,
                    SkillDiagnosticCode::NulByte,
                    SkillDiagnosticSeverity::Error,
                    "SKILL.md contains a NUL byte.",
                ));
                continue;
            }
            let contents = match std::str::from_utf8(&bytes) {
                Ok(contents) => contents,
                Err(_) => {
                    diagnostics.push(diagnostic(
                        &canonical_skill_file,
                        SkillDiagnosticCode::InvalidUtf8,
                        SkillDiagnosticSeverity::Error,
                        "SKILL.md must be valid UTF-8.",
                    ));
                    continue;
                }
            };
            let metadata = match parse_skill_metadata(contents, directory_name) {
                Ok(metadata) => metadata,
                Err(error) => {
                    diagnostics.push(diagnostic(
                        &canonical_skill_file,
                        error.diagnostic_code(),
                        SkillDiagnosticSeverity::Error,
                        error.to_string(),
                    ));
                    continue;
                }
            };
            if metadata.name_was_defaulted {
                diagnostics.push(diagnostic(
                    &canonical_skill_file,
                    SkillDiagnosticCode::DefaultedName,
                    SkillDiagnosticSeverity::Warning,
                    format!(
                        "Skill frontmatter has no name; using directory name `{directory_name}`."
                    ),
                ));
            }

            let Some(path) = canonical_skill_file.to_str() else {
                diagnostics.push(diagnostic(
                    &canonical_skill_file,
                    SkillDiagnosticCode::UnsupportedPathEncoding,
                    SkillDiagnosticSeverity::Error,
                    "The canonical SKILL.md path cannot be represented as UTF-8.",
                ));
                continue;
            };
            let relative_path = relative_display(&workspace_root, &canonical_skill_file);
            skills.push(SkillDescriptor {
                id: format!(
                    "workspace:{}:{}",
                    percent_encode(workspace_id.as_bytes()),
                    percent_encode(directory_name.as_bytes())
                ),
                name: metadata.name,
                description: metadata.description,
                scope: SkillScope::Workspace,
                path: path.to_owned(),
                relative_path,
                revision: content_revision(&bytes),
            });
        }

        skills.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.id.cmp(&right.id))
        });
        append_duplicate_name_diagnostics(&skills, &canonical_skills_root, &mut diagnostics);

        Ok(finalize_catalog(skills, diagnostics, catalog_truncated))
    }
}

#[derive(Debug)]
struct ScanBudget {
    remaining_entries: usize,
}

impl ScanBudget {
    fn new(max_entries: usize) -> Self {
        Self {
            remaining_entries: max_entries,
        }
    }

    fn consume_entry(&mut self) -> bool {
        if self.remaining_entries == 0 {
            return false;
        }
        self.remaining_entries -= 1;
        true
    }
}

fn collect_skill_directories(
    root: &Path,
    scan_budget: &mut ScanBudget,
) -> (Vec<PathBuf>, Vec<SkillDiagnostic>, bool) {
    let mut diagnostics = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            diagnostics.push(diagnostic(
                root,
                SkillDiagnosticCode::InvalidRoot,
                SkillDiagnosticSeverity::Error,
                format!("Cannot read workspace Skill root: {error}"),
            ));
            return (Vec::new(), diagnostics, false);
        }
    };

    let mut collected = Vec::new();
    let mut observed_entries = 0usize;
    for entry in entries {
        observed_entries = observed_entries.saturating_add(1);
        if !scan_budget.consume_entry() {
            diagnostics.push(diagnostic(
                root,
                SkillDiagnosticCode::ScanBudgetExceeded,
                SkillDiagnosticSeverity::Warning,
                format!(
                    "Workspace Skill discovery exceeded its {MAX_SKILL_SCAN_ENTRIES}-entry scan budget."
                ),
            ));
            return (Vec::new(), diagnostics, true);
        }
        if observed_entries > MAX_SKILL_ROOT_ENTRIES {
            diagnostics.push(diagnostic(
                root,
                SkillDiagnosticCode::TooManyEntries,
                SkillDiagnosticSeverity::Warning,
                format!(
                    "Workspace Skill root contains more than {MAX_SKILL_ROOT_ENTRIES} entries."
                ),
            ));
            return (Vec::new(), diagnostics, true);
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                diagnostics.push(diagnostic(
                    root,
                    SkillDiagnosticCode::UnreadableEntry,
                    SkillDiagnosticSeverity::Error,
                    format!("Cannot read an entry in the workspace Skill root: {error}"),
                ));
                continue;
            }
        };
        collected.push(entry);
    }

    collected.sort_by_key(|entry| entry.path());
    let mut candidates = Vec::new();
    for entry in collected {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with('.'))
        {
            continue;
        }
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                diagnostics.push(diagnostic(
                    &path,
                    SkillDiagnosticCode::UnreadableEntry,
                    SkillDiagnosticSeverity::Error,
                    format!("Cannot inspect workspace Skill entry: {error}"),
                ));
                continue;
            }
        };
        if file_type.is_symlink() {
            diagnostics.push(diagnostic(
                &path,
                SkillDiagnosticCode::SymlinkNotAllowed,
                SkillDiagnosticSeverity::Error,
                "Workspace Skill discovery does not follow symlinked Skill directories.",
            ));
        } else if file_type.is_dir() {
            candidates.push(path);
        }
    }
    (candidates, diagnostics, false)
}

enum ExactSkillFile {
    Found(PathBuf),
    Missing,
    TooManyEntries,
    ScanBudgetExceeded,
}

fn find_exact_skill_file(
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

enum BoundedReadError {
    Io(io::Error),
    TooLarge,
    CatalogBudgetExceeded,
    PathChanged(String),
}

#[derive(Debug)]
struct ByteBudget {
    remaining_bytes: usize,
}

impl ByteBudget {
    fn new(max_bytes: usize) -> Self {
        Self {
            remaining_bytes: max_bytes,
        }
    }

    fn consume(&mut self, bytes: usize) {
        self.remaining_bytes = self.remaining_bytes.saturating_sub(bytes);
    }
}

fn read_bounded_verified(
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
fn verify_opened_file_identity(
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
fn verify_opened_file_identity(
    file: &File,
    _expected: &fs::Metadata,
    _opened: &fs::Metadata,
    expected_canonical_path: &Path,
) -> Result<(), String> {
    let opened_identity = windows_file_identity(file)
        .map_err(|error| format!("cannot identify the opened file: {error}"))?;
    let opened_path = windows_final_path(file)
        .map_err(|error| format!("cannot resolve the opened file handle: {error}"))?;
    if !windows_paths_equivalent(&opened_path, expected_canonical_path) {
        return Err("the opened handle resolves outside the checked workspace path".to_string());
    }

    let expected_file = open_skill_file(expected_canonical_path)
        .map_err(|error| format!("cannot reopen the checked canonical path: {error}"))?;
    let expected_identity = windows_file_identity(&expected_file)
        .map_err(|error| format!("cannot identify the checked canonical file: {error}"))?;
    if opened_identity != expected_identity {
        return Err("the opened file is not the file that was checked".to_string());
    }
    Ok(())
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
fn normalize_windows_path_units(mut units: Vec<u16>) -> Vec<u16> {
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
fn verify_opened_file_identity(
    _file: &File,
    _expected: &fs::Metadata,
    _opened: &fs::Metadata,
    _expected_canonical_path: &Path,
) -> Result<(), String> {
    Err("secure Skill file identity checks are unavailable on this platform".to_string())
}

fn read_open_file_bounded(
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

fn append_duplicate_name_diagnostics(
    skills: &[SkillDescriptor],
    skills_root: &Path,
    diagnostics: &mut Vec<SkillDiagnostic>,
) {
    let mut paths_by_name = BTreeMap::<&str, Vec<&str>>::new();
    for skill in skills {
        paths_by_name
            .entry(&skill.name)
            .or_default()
            .push(&skill.relative_path);
    }
    for (name, paths) in paths_by_name {
        if paths.len() < 2 {
            continue;
        }
        diagnostics.push(diagnostic(
            skills_root,
            SkillDiagnosticCode::DuplicateName,
            SkillDiagnosticSeverity::Warning,
            format!(
                "Skill name `{name}` is declared by multiple paths: {}. Explicit selection must use Skill id.",
                paths.join(", ")
            ),
        ));
    }
}

fn metadata_if_present(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn catalog_with_root_diagnostic(
    path: &Path,
    code: SkillDiagnosticCode,
    message: impl Into<String>,
) -> SkillCatalog {
    finalize_catalog(
        Vec::new(),
        vec![diagnostic(
            path,
            code,
            SkillDiagnosticSeverity::Error,
            message,
        )],
        false,
    )
}

fn diagnostic(
    path: &Path,
    code: SkillDiagnosticCode,
    severity: SkillDiagnosticSeverity,
    message: impl Into<String>,
) -> SkillDiagnostic {
    SkillDiagnostic {
        code,
        severity,
        message: message.into(),
        path: diagnostic_path(path),
    }
}

fn diagnostic_path(path: &Path) -> String {
    if let Some(path) = path.to_str() {
        return path.to_owned();
    }
    diagnostic_path_non_utf8(path)
}

#[cfg(unix)]
fn diagnostic_path_non_utf8(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;

    format!("unix-bytes:{}", percent_encode(path.as_os_str().as_bytes()))
}

#[cfg(windows)]
fn diagnostic_path_non_utf8(path: &Path) -> String {
    use std::fmt::Write;
    use std::os::windows::ffi::OsStrExt;

    let mut encoded = String::from("windows-wide:");
    for unit in path.as_os_str().encode_wide() {
        write!(&mut encoded, "%u{unit:04X}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(not(any(unix, windows)))]
fn diagnostic_path_non_utf8(path: &Path) -> String {
    format!("platform-path:{:?}", path.as_os_str())
}

fn finalize_catalog(
    skills: Vec<SkillDescriptor>,
    mut diagnostics: Vec<SkillDiagnostic>,
    truncated: bool,
) -> SkillCatalog {
    diagnostics.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.code.stable_name().cmp(right.code.stable_name()))
            .then_with(|| left.message.cmp(&right.message))
    });
    let revision_material = serde_json::to_vec(&(&skills, &diagnostics, truncated))
        .expect("Skill catalog revision material must serialize");
    SkillCatalog {
        catalog_revision: content_revision(&revision_material),
        skills,
        diagnostics,
        truncated,
    }
}

fn relative_display(root: &Path, path: &Path) -> String {
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

fn percent_encode(bytes: &[u8]) -> String {
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
    use std::fs;
    use tempfile::tempdir;

    fn valid_skill(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# Instructions\n")
    }

    fn write_skill(workspace: &Path, directory: &str, contents: &[u8]) -> PathBuf {
        let skill_directory = workspace
            .join(AGENTS_DIRECTORY)
            .join(SKILLS_DIRECTORY)
            .join(directory);
        fs::create_dir_all(&skill_directory).unwrap();
        let path = skill_directory.join(SKILL_FILE_NAME);
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn discovers_the_repository_auditor_fixture() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace");

        let catalog = SkillsService::new()
            .list_workspace("fixture-workspace", &fixture)
            .unwrap();

        assert!(!catalog.truncated);
        assert!(catalog.diagnostics.is_empty());
        assert_eq!(catalog.skills.len(), 1);
        let skill = &catalog.skills[0];
        assert_eq!(
            skill.id,
            "workspace:fixture-workspace:repository-evidence-auditor"
        );
        assert_eq!(skill.name, "repository-evidence-auditor");
        assert_eq!(skill.scope, SkillScope::Workspace);
        assert_eq!(
            skill.relative_path,
            ".agents/skills/repository-evidence-auditor/SKILL.md"
        );
        assert!(!skill.revision.is_empty());
    }

    #[test]
    fn missing_skill_root_returns_a_stable_empty_catalog() {
        let workspace = tempdir().unwrap();
        let service = SkillsService::new();

        let first = service
            .list_workspace("workspace", workspace.path())
            .unwrap();
        let second = service
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert!(first.skills.is_empty());
        assert!(first.diagnostics.is_empty());
        assert_eq!(first.catalog_revision, second.catalog_revision);
    }

    #[test]
    fn isolates_invalid_skills_and_sorts_valid_skills() {
        let workspace = tempdir().unwrap();
        write_skill(
            workspace.path(),
            "zeta",
            valid_skill("zeta", "Zeta skill.").as_bytes(),
        );
        write_skill(workspace.path(), "broken", b"not frontmatter");
        write_skill(
            workspace.path(),
            "alpha",
            valid_skill("alpha", "Alpha skill.").as_bytes(),
        );

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert_eq!(
            catalog
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );
        assert_eq!(catalog.diagnostics.len(), 1);
        assert_eq!(
            catalog.diagnostics[0].code,
            SkillDiagnosticCode::MissingFrontmatter
        );
    }

    #[test]
    fn revision_tracks_the_exact_skill_bytes_while_id_stays_path_based() {
        let workspace = tempdir().unwrap();
        let first_contents = concat!(
            "---\n",
            "name: auditor\n",
            "description: Audit a repository.\n",
            "---\n",
            "# Instructions\n",
            "Version one.\n"
        );
        let skill_path = write_skill(workspace.path(), "auditor", first_contents.as_bytes());
        let service = SkillsService::new();
        let first = service
            .list_workspace("workspace", workspace.path())
            .unwrap();

        fs::write(
            skill_path,
            first_contents.replace("Version one.", "Version two."),
        )
        .unwrap();
        let second = service
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert_eq!(first.skills[0].id, second.skills[0].id);
        assert_ne!(first.skills[0].revision, second.skills[0].revision);
        assert_ne!(first.catalog_revision, second.catalog_revision);
    }

    #[test]
    fn skill_ids_are_scoped_to_the_project_identity() {
        let root = tempdir().unwrap();
        let first_workspace = root.path().join("first");
        let second_workspace = root.path().join("second");
        let contents = valid_skill("auditor", "Audit a repository.");
        write_skill(&first_workspace, "auditor", contents.as_bytes());
        write_skill(&second_workspace, "auditor", contents.as_bytes());
        let service = SkillsService::new();

        let first = service
            .list_workspace("project:one", &first_workspace)
            .unwrap();
        let second = service
            .list_workspace("project:two", &second_workspace)
            .unwrap();

        assert_eq!(first.skills[0].id, "workspace:project%3Aone:auditor");
        assert_eq!(second.skills[0].id, "workspace:project%3Atwo:auditor");
        assert_ne!(first.skills[0].id, second.skills[0].id);
        assert_eq!(first.skills[0].revision, second.skills[0].revision);
    }

    #[test]
    fn preserves_duplicate_names_without_overwriting() {
        let workspace = tempdir().unwrap();
        write_skill(
            workspace.path(),
            "first",
            valid_skill("shared-name", "First path.").as_bytes(),
        );
        write_skill(
            workspace.path(),
            "second",
            valid_skill("shared-name", "Second path.").as_bytes(),
        );

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert_eq!(catalog.skills.len(), 2);
        assert_ne!(catalog.skills[0].id, catalog.skills[1].id);
        assert!(catalog
            .diagnostics
            .iter()
            .any(|item| item.code == SkillDiagnosticCode::DuplicateName));
    }

    #[test]
    fn rejects_wrong_case_invalid_utf8_nul_and_oversized_files() {
        let workspace = tempdir().unwrap();
        let wrong_case = workspace
            .path()
            .join(AGENTS_DIRECTORY)
            .join(SKILLS_DIRECTORY)
            .join("wrong-case");
        fs::create_dir_all(&wrong_case).unwrap();
        fs::write(
            wrong_case.join("skill.md"),
            valid_skill("wrong-case", "Wrong case."),
        )
        .unwrap();
        write_skill(workspace.path(), "invalid-utf8", &[0xff, 0xfe]);
        write_skill(
            workspace.path(),
            "nul-byte",
            b"---\nname: nul-byte\ndescription: Invalid.\n---\n\0",
        );
        write_skill(
            workspace.path(),
            "oversized",
            &vec![b'x'; MAX_SKILL_FILE_BYTES + 1],
        );

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();
        let codes = catalog
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();

        assert!(codes.contains(&SkillDiagnosticCode::MissingSkillFile));
        assert!(codes.contains(&SkillDiagnosticCode::InvalidUtf8));
        assert!(codes.contains(&SkillDiagnosticCode::NulByte));
        assert!(codes.contains(&SkillDiagnosticCode::SkillFileTooLarge));
        assert!(catalog.skills.is_empty());
    }

    #[test]
    fn accepts_a_skill_file_exactly_at_the_size_limit() {
        let workspace = tempdir().unwrap();
        let mut contents = valid_skill("at-limit", "Exactly at the byte limit.").into_bytes();
        contents.resize(MAX_SKILL_FILE_BYTES, b' ');
        write_skill(workspace.path(), "at-limit", &contents);

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert_eq!(catalog.skills.len(), 1);
        assert!(catalog.diagnostics.is_empty());
    }

    #[test]
    fn defaults_missing_name_but_reports_a_warning() {
        let workspace = tempdir().unwrap();
        write_skill(
            workspace.path(),
            "fallback-name",
            b"---\ndescription: Uses its directory name.\n---\n",
        );

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert_eq!(catalog.skills[0].name, "fallback-name");
        assert_eq!(
            catalog.diagnostics[0].code,
            SkillDiagnosticCode::DefaultedName
        );
        assert_eq!(
            catalog.diagnostics[0].severity,
            SkillDiagnosticSeverity::Warning
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_skill_directories_and_files() {
        use std::os::unix::fs::symlink;

        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_skill = outside.path().join(SKILL_FILE_NAME);
        fs::write(
            &outside_skill,
            valid_skill("outside", "Must not be loaded."),
        )
        .unwrap();

        let skills_root = workspace
            .path()
            .join(AGENTS_DIRECTORY)
            .join(SKILLS_DIRECTORY);
        fs::create_dir_all(&skills_root).unwrap();
        symlink(outside.path(), skills_root.join("linked-directory")).unwrap();
        let linked_file_directory = skills_root.join("linked-file");
        fs::create_dir(&linked_file_directory).unwrap();
        symlink(&outside_skill, linked_file_directory.join(SKILL_FILE_NAME)).unwrap();

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert!(catalog.skills.is_empty());
        assert_eq!(
            catalog
                .diagnostics
                .iter()
                .filter(|item| item.code == SkillDiagnosticCode::SymlinkNotAllowed)
                .count(),
            2
        );
        assert!(!catalog
            .diagnostics
            .iter()
            .any(|item| item.message.contains("Must not be loaded")));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_agents_root() {
        use std::os::unix::fs::symlink;

        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        symlink(outside.path(), workspace.path().join(AGENTS_DIRECTORY)).unwrap();

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert!(catalog.skills.is_empty());
        assert_eq!(catalog.diagnostics.len(), 1);
        assert_eq!(
            catalog.diagnostics[0].code,
            SkillDiagnosticCode::SymlinkNotAllowed
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_skills_root() {
        use std::os::unix::fs::symlink;

        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let agents_root = workspace.path().join(AGENTS_DIRECTORY);
        fs::create_dir(&agents_root).unwrap();
        symlink(outside.path(), agents_root.join(SKILLS_DIRECTORY)).unwrap();

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert!(catalog.skills.is_empty());
        assert_eq!(catalog.diagnostics.len(), 1);
        assert_eq!(
            catalog.diagnostics[0].code,
            SkillDiagnosticCode::SymlinkNotAllowed
        );
    }

    #[cfg(unix)]
    #[test]
    fn losslessly_distinguishes_non_utf8_diagnostic_paths() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let first_path = PathBuf::from("/workspace").join(OsString::from_vec(vec![b'a', 0xff]));
        let second_path = PathBuf::from("/workspace").join(OsString::from_vec(vec![b'a', 0xfe]));
        let first_diagnostic = diagnostic(
            &first_path,
            SkillDiagnosticCode::UnsupportedPathEncoding,
            SkillDiagnosticSeverity::Error,
            "unsupported path",
        );
        let second_diagnostic = diagnostic(
            &second_path,
            SkillDiagnosticCode::UnsupportedPathEncoding,
            SkillDiagnosticSeverity::Error,
            "unsupported path",
        );
        let first_catalog = finalize_catalog(Vec::new(), vec![first_diagnostic.clone()], false);
        let second_catalog = finalize_catalog(Vec::new(), vec![second_diagnostic.clone()], false);

        assert!(first_diagnostic.path.starts_with("unix-bytes:"));
        assert!(second_diagnostic.path.starts_with("unix-bytes:"));
        assert_ne!(first_diagnostic.path, second_diagnostic.path);
        assert_ne!(
            first_catalog.catalog_revision,
            second_catalog.catalog_revision
        );
    }

    #[test]
    fn exact_file_lookup_consumes_a_global_entry_budget() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join(SKILL_FILE_NAME), "skill").unwrap();
        fs::write(directory.path().join("reference.md"), "reference").unwrap();
        let mut budget = ScanBudget::new(1);

        let result = find_exact_skill_file(directory.path(), &mut budget).unwrap();

        assert!(matches!(result, ExactSkillFile::ScanBudgetExceeded));
    }

    #[cfg(unix)]
    #[test]
    fn verified_read_rejects_a_file_replaced_after_validation() {
        let workspace = tempdir().unwrap();
        let skill_path = write_skill(
            workspace.path(),
            "auditor",
            valid_skill("auditor", "Original bytes.").as_bytes(),
        );
        let expected_metadata = fs::symlink_metadata(&skill_path).unwrap();
        let expected_canonical_path = skill_path.canonicalize().unwrap();
        let canonical_skill_directory = skill_path.parent().unwrap().canonicalize().unwrap();
        let canonical_workspace_root = workspace.path().canonicalize().unwrap();
        fs::rename(&skill_path, skill_path.with_extension("checked")).unwrap();
        fs::write(&skill_path, valid_skill("auditor", "Replacement bytes.")).unwrap();
        let mut byte_budget = ByteBudget::new(MAX_SKILL_CATALOG_BYTES);

        let result = read_bounded_verified(
            &skill_path,
            &expected_canonical_path,
            &expected_metadata,
            &canonical_skill_directory,
            &canonical_workspace_root,
            MAX_SKILL_FILE_BYTES,
            &mut byte_budget,
        );

        assert!(matches!(result, Err(BoundedReadError::PathChanged(_))));
    }

    #[test]
    fn catalog_byte_budget_counts_bytes_read_from_oversized_files() {
        let directory = tempdir().unwrap();
        let first_path = directory.path().join("first");
        let second_path = directory.path().join("second");
        fs::write(&first_path, b"12345").unwrap();
        fs::write(&second_path, b"12345").unwrap();
        let mut byte_budget = ByteBudget::new(6);

        let first = read_open_file_bounded(File::open(first_path).unwrap(), 3, &mut byte_budget);
        assert!(matches!(first, Err(BoundedReadError::TooLarge)));
        assert_eq!(byte_budget.remaining_bytes, 2);

        let second = read_open_file_bounded(File::open(second_path).unwrap(), 3, &mut byte_budget);
        assert_eq!(byte_budget.remaining_bytes, 0);
        assert!(matches!(
            second,
            Err(BoundedReadError::CatalogBudgetExceeded)
        ));
    }

    #[test]
    fn normalizes_windows_verbatim_drive_and_unc_paths_for_handle_comparison() {
        let drive = normalize_windows_path_units(
            r"\\?\C:\Workspace\Skills\Audit\SKILL.md"
                .encode_utf16()
                .collect(),
        );
        let regular_drive = normalize_windows_path_units(
            r"c:\workspace\skills\audit\skill.md"
                .encode_utf16()
                .collect(),
        );
        let unc = normalize_windows_path_units(
            r"\\?\UNC\Server\Share\Skills\SKILL.md"
                .encode_utf16()
                .collect(),
        );
        let regular_unc = normalize_windows_path_units(
            r"\\server\share\skills\skill.md".encode_utf16().collect(),
        );

        assert_eq!(drive, regular_drive);
        assert_eq!(unc, regular_unc);
    }
}
