//! Validated, immutable Skill package snapshots.
//!
//! Acquisition is deliberately separate from installation. Local directories
//! are the first adapter; future Git, URL, ZIP, or registry adapters must also
//! end by constructing this same fully validated byte snapshot before they may
//! call the managed-store installer.

use super::digest::package_revision;
use super::model::{SkillDiagnosticCode, SkillRevision};
use super::origin::SkillPackageOrigin;
use super::parser::parse_skill_document;
use super::workspace::{
    is_symlink_or_reparse, metadata_if_present, read_bounded_verified, verify_opened_file_identity,
    verify_plain_directory, BoundedReadError, ByteBudget, MAX_SKILL_FILE_BYTES, SKILL_FILE_NAME,
};
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
};

pub const LOCAL_DIRECTORY_SKILL_ORIGIN_PROVIDER: &str = "local-directory";

/// An exact package byte snapshot that has passed all package-v1 validation.
///
/// The type has no unchecked constructor and exposes no mutable contents. Once
/// prepared, installation never reads the acquisition path again.
#[derive(Clone, PartialEq, Eq)]
pub struct PreparedSkillPackage {
    source: Arc<str>,
    revision: SkillRevision,
    name: String,
    description: String,
    instructions_range: Range<usize>,
    origin: SkillPackageOrigin,
}

impl fmt::Debug for PreparedSkillPackage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedSkillPackage")
            .field("revision", &self.revision)
            .field("name", &self.name)
            .field("source_bytes", &self.source.len())
            .field("instructions_range", &self.instructions_range)
            .field("origin", &self.origin)
            .finish()
    }
}

impl PreparedSkillPackage {
    /// Central validation entry point for future acquisition adapters.
    pub fn from_bytes(
        bytes: Vec<u8>,
        origin: SkillPackageOrigin,
    ) -> Result<Self, SkillPackagePreparationError> {
        let validated = validate_installable_skill_bytes(bytes)
            .map_err(|error| SkillPackagePreparationError::new(error.code, error.message))?;
        let revision = package_revision(validated.source.as_bytes());
        Ok(Self {
            source: validated.source,
            revision,
            name: validated.name,
            description: validated.description,
            instructions_range: validated.instructions_range,
            origin,
        })
    }

    /// Imports a package-v1 local directory containing only exact-case
    /// `SKILL.md`, then captures and validates its bytes.
    ///
    /// The returned package does not retain or trust the directory: all model
    /// input comes from the owned, validated byte snapshot. The caller must
    /// still treat the acquisition path as cooperatively owned while this
    /// method runs. Identity checks reject ordinary replacement races, but the
    /// audit-only origin reference is not a cryptographic binding to a path
    /// controlled by a concurrently adversarial local process.
    pub fn from_local_directory(
        directory: impl AsRef<Path>,
        reference: impl Into<String>,
    ) -> Result<Self, SkillPackagePreparationError> {
        let origin = SkillPackageOrigin::new(LOCAL_DIRECTORY_SKILL_ORIGIN_PROVIDER, reference)
            .map_err(|error| {
                SkillPackagePreparationError::new(
                    SkillDiagnosticCode::SourceContractViolation,
                    format!("Invalid local-directory origin reference: {error}"),
                )
            })?;
        let bytes = read_local_skill_directory(directory.as_ref())?;
        Self::from_bytes(bytes, origin)
    }

    pub fn revision(&self) -> &SkillRevision {
        &self.revision
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn origin(&self) -> &SkillPackageOrigin {
        &self.origin
    }

    pub fn source_bytes(&self) -> &[u8] {
        self.source.as_bytes()
    }

    pub fn instructions(&self) -> &str {
        &self.source[self.instructions_range.clone()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SkillPackagePreparationError {
    code: SkillDiagnosticCode,
    reason: String,
}

impl SkillPackagePreparationError {
    fn new(code: SkillDiagnosticCode, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
        }
    }

    pub fn code(&self) -> SkillDiagnosticCode {
        self.code
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillPackagePreparationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillPackagePreparationError {}

#[derive(Debug)]
pub(super) struct ValidatedSkillDocument {
    pub source: Arc<str>,
    pub name: String,
    pub description: String,
    pub instructions_range: Range<usize>,
}

#[derive(Debug)]
pub(super) struct SkillPackageContentError {
    pub code: SkillDiagnosticCode,
    pub message: String,
}

pub(super) fn validate_installable_skill_bytes(
    bytes: Vec<u8>,
) -> Result<ValidatedSkillDocument, SkillPackageContentError> {
    if bytes.len() > MAX_SKILL_FILE_BYTES {
        return Err(content_error(
            SkillDiagnosticCode::SkillFileTooLarge,
            format!("SKILL.md exceeds {MAX_SKILL_FILE_BYTES} bytes."),
        ));
    }
    if bytes.contains(&0) {
        return Err(content_error(
            SkillDiagnosticCode::NulByte,
            "SKILL.md contains a NUL byte.",
        ));
    }
    let source = String::from_utf8(bytes).map_err(|_| {
        content_error(
            SkillDiagnosticCode::InvalidUtf8,
            "SKILL.md must be valid UTF-8.",
        )
    })?;
    let document = parse_skill_document(&source, "").map_err(|error| {
        content_error(
            error.diagnostic_code(),
            format!("Invalid Skill document: {error}"),
        )
    })?;
    if document.metadata.name_was_defaulted {
        return Err(content_error(
            SkillDiagnosticCode::InvalidName,
            "User-installed Skills must declare an explicit frontmatter name.",
        ));
    }
    Ok(ValidatedSkillDocument {
        source: Arc::from(source),
        name: document.metadata.name,
        description: document.metadata.description,
        instructions_range: document.instructions_range,
    })
}

fn content_error(
    code: SkillDiagnosticCode,
    message: impl Into<String>,
) -> SkillPackageContentError {
    SkillPackageContentError {
        code,
        message: message.into(),
    }
}

fn read_local_skill_directory(directory: &Path) -> Result<Vec<u8>, SkillPackagePreparationError> {
    if !directory.is_absolute() {
        return Err(SkillPackagePreparationError::new(
            SkillDiagnosticCode::InvalidRoot,
            "Local Skill directory must be an absolute path.",
        ));
    }
    let metadata = metadata_if_present(directory)
        .map_err(|error| preparation_io("Cannot inspect local Skill directory", error))?
        .ok_or_else(|| {
            SkillPackagePreparationError::new(
                SkillDiagnosticCode::InvalidRoot,
                "Local Skill directory does not exist.",
            )
        })?;
    if is_symlink_or_reparse(&metadata) {
        return Err(SkillPackagePreparationError::new(
            SkillDiagnosticCode::SymlinkNotAllowed,
            "Local Skill directory cannot be a symlink or reparse point.",
        ));
    }
    if !metadata.is_dir() {
        return Err(SkillPackagePreparationError::new(
            SkillDiagnosticCode::InvalidRoot,
            "Local Skill acquisition path is not a directory.",
        ));
    }
    let canonical_directory = directory
        .canonicalize()
        .map_err(|error| preparation_io("Cannot resolve local Skill directory", error))?;
    verify_plain_directory(
        directory,
        &canonical_directory,
        "The local Skill directory changed while it was inspected.",
    )
    .map_err(|issue| SkillPackagePreparationError::new(issue.code, issue.message))?;
    let directory_handle = open_local_directory(directory, &metadata, &canonical_directory)?;

    let skill_path = exact_local_skill_path(directory)?;
    let file_metadata = metadata_if_present(&skill_path)
        .map_err(|error| preparation_io("Cannot inspect local SKILL.md", error))?
        .ok_or_else(|| {
            SkillPackagePreparationError::new(
                SkillDiagnosticCode::PathChangedDuringRead,
                "Local SKILL.md disappeared while it was inspected.",
            )
        })?;
    if is_symlink_or_reparse(&file_metadata) {
        return Err(SkillPackagePreparationError::new(
            SkillDiagnosticCode::SymlinkNotAllowed,
            "Local Skill acquisition does not follow a symlinked SKILL.md.",
        ));
    }
    if !file_metadata.is_file() {
        return Err(SkillPackagePreparationError::new(
            SkillDiagnosticCode::MissingSkillFile,
            "Local SKILL.md is not a regular file.",
        ));
    }
    let canonical_skill_file = skill_path
        .canonicalize()
        .map_err(|error| preparation_io("Cannot resolve local SKILL.md", error))?;
    if !canonical_skill_file.starts_with(&canonical_directory) {
        return Err(SkillPackagePreparationError::new(
            SkillDiagnosticCode::RootEscapesWorkspace,
            "Local SKILL.md resolves outside its acquisition directory.",
        ));
    }
    let mut budget = ByteBudget::new(MAX_SKILL_FILE_BYTES.saturating_add(1));
    let bytes = read_bounded_verified(
        &skill_path,
        &canonical_skill_file,
        &file_metadata,
        &canonical_directory,
        &canonical_directory,
        MAX_SKILL_FILE_BYTES,
        &mut budget,
    )
    .map_err(|error| match error {
        BoundedReadError::TooLarge | BoundedReadError::CatalogBudgetExceeded => {
            SkillPackagePreparationError::new(
                SkillDiagnosticCode::SkillFileTooLarge,
                format!("SKILL.md exceeds {MAX_SKILL_FILE_BYTES} bytes."),
            )
        }
        BoundedReadError::Io(error) => preparation_io("Cannot read local SKILL.md", error),
        BoundedReadError::PathChanged(reason) => SkillPackagePreparationError::new(
            SkillDiagnosticCode::PathChangedDuringRead,
            format!("Local SKILL.md changed while it was read: {reason}"),
        ),
    })?;
    verify_plain_directory(
        directory,
        &canonical_directory,
        "The local Skill directory changed while SKILL.md was read.",
    )
    .map_err(|issue| SkillPackagePreparationError::new(issue.code, issue.message))?;
    if exact_local_skill_path(directory)? != skill_path {
        return Err(SkillPackagePreparationError::new(
            SkillDiagnosticCode::PathChangedDuringRead,
            "The local SKILL.md path changed while it was read.",
        ));
    }
    verify_local_directory_binding(&directory_handle, directory, &canonical_directory)?;
    Ok(bytes)
}

fn open_local_directory(
    directory: &Path,
    expected_metadata: &fs::Metadata,
    canonical_directory: &Path,
) -> Result<File, SkillPackagePreparationError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY);
    #[cfg(windows)]
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS);
    let directory_handle = options
        .open(directory)
        .map_err(|error| preparation_io("Cannot safely open local Skill directory", error))?;
    let opened_metadata = directory_handle
        .metadata()
        .map_err(|error| preparation_io("Cannot inspect opened local Skill directory", error))?;
    if !opened_metadata.is_dir() {
        return Err(SkillPackagePreparationError::new(
            SkillDiagnosticCode::PathChangedDuringRead,
            "The opened local Skill acquisition object is not a directory.",
        ));
    }
    verify_opened_file_identity(
        &directory_handle,
        expected_metadata,
        &opened_metadata,
        canonical_directory,
    )
    .map_err(|reason| {
        SkillPackagePreparationError::new(
            SkillDiagnosticCode::PathChangedDuringRead,
            format!("Local Skill directory changed while it was opened: {reason}"),
        )
    })?;
    Ok(directory_handle)
}

fn verify_local_directory_binding(
    directory_handle: &File,
    directory: &Path,
    expected_canonical: &Path,
) -> Result<(), SkillPackagePreparationError> {
    let current_metadata = fs::symlink_metadata(directory)
        .map_err(|error| preparation_io("Cannot re-inspect local Skill directory", error))?;
    if is_symlink_or_reparse(&current_metadata) || !current_metadata.is_dir() {
        return Err(SkillPackagePreparationError::new(
            SkillDiagnosticCode::PathChangedDuringRead,
            "The local Skill directory changed while SKILL.md was read.",
        ));
    }
    let current_canonical = directory
        .canonicalize()
        .map_err(|error| preparation_io("Cannot re-resolve local Skill directory", error))?;
    if current_canonical != expected_canonical {
        return Err(SkillPackagePreparationError::new(
            SkillDiagnosticCode::PathChangedDuringRead,
            "The local Skill directory resolved to a different path while SKILL.md was read.",
        ));
    }
    let opened_metadata = directory_handle
        .metadata()
        .map_err(|error| preparation_io("Cannot re-inspect opened local Skill directory", error))?;
    verify_opened_file_identity(
        directory_handle,
        &current_metadata,
        &opened_metadata,
        expected_canonical,
    )
    .map_err(|reason| {
        SkillPackagePreparationError::new(
            SkillDiagnosticCode::PathChangedDuringRead,
            format!("Local Skill directory identity changed while SKILL.md was read: {reason}"),
        )
    })
}

fn exact_local_skill_path(
    directory: &Path,
) -> Result<std::path::PathBuf, SkillPackagePreparationError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| preparation_io("Cannot read local Skill directory", error))?;
    let entry = match entries.next() {
        Some(entry) => entry
            .map_err(|error| preparation_io("Cannot inspect local Skill directory entry", error))?,
        None => {
            return Err(SkillPackagePreparationError::new(
                SkillDiagnosticCode::MissingSkillFile,
                "Local Skill directory does not contain an exact-case SKILL.md file.",
            ))
        }
    };
    if entry.file_name() != OsStr::new(SKILL_FILE_NAME) {
        return Err(SkillPackagePreparationError::new(
            SkillDiagnosticCode::UnexpectedPackageEntry,
            "Package format v1 permits only an exact-case SKILL.md file.",
        ));
    }
    if let Some(entry) = entries.next() {
        entry
            .map_err(|error| preparation_io("Cannot inspect local Skill directory entry", error))?;
        return Err(SkillPackagePreparationError::new(
            SkillDiagnosticCode::UnexpectedPackageEntry,
            "Package format v1 permits no sibling resources or additional entries.",
        ));
    }
    Ok(entry.path())
}

fn preparation_io(action: &str, error: std::io::Error) -> SkillPackagePreparationError {
    SkillPackagePreparationError::new(
        SkillDiagnosticCode::UnreadableEntry,
        format!("{action}: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn origin() -> SkillPackageOrigin {
        SkillPackageOrigin::new("local-directory", "fixture").unwrap()
    }

    fn document(name: Option<&str>, instructions: &str) -> String {
        let name = name
            .map(|name| format!("name: {name}\n"))
            .unwrap_or_default();
        format!("---\n{name}description: Prepared fixture.\n---\n# Instructions\n{instructions}\n")
    }

    #[test]
    fn local_directory_produces_an_owned_verified_snapshot() {
        let fixture = tempdir().unwrap();
        let directory = fixture.path().join("skill");
        fs::create_dir(&directory).unwrap();
        fs::write(
            directory.join(SKILL_FILE_NAME),
            document(Some("prepared"), "ORIGINAL"),
        )
        .unwrap();

        let package = PreparedSkillPackage::from_local_directory(&directory, "fixture").unwrap();
        fs::write(
            directory.join(SKILL_FILE_NAME),
            document(Some("prepared"), "MUTATED"),
        )
        .unwrap();

        assert_eq!(package.name(), "prepared");
        assert!(package.instructions().contains("ORIGINAL"));
        assert!(!package.instructions().contains("MUTATED"));
        assert_eq!(
            package.revision(),
            &package_revision(package.source_bytes())
        );
        assert_eq!(
            package.origin().provider(),
            LOCAL_DIRECTORY_SKILL_ORIGIN_PROVIDER
        );
        assert_eq!(package.origin().reference(), "fixture");
        let debug = format!("{package:?}");
        assert!(!debug.contains("ORIGINAL"));
        assert!(!debug.contains("fixture"));
    }

    #[test]
    fn preparation_rejects_invalid_content_and_defaulted_names() {
        let invalid_utf8 = PreparedSkillPackage::from_bytes(vec![0xff], origin()).unwrap_err();
        assert_eq!(invalid_utf8.code(), SkillDiagnosticCode::InvalidUtf8);
        let nul = PreparedSkillPackage::from_bytes(b"---\0".to_vec(), origin()).unwrap_err();
        assert_eq!(nul.code(), SkillDiagnosticCode::NulByte);
        let defaulted =
            PreparedSkillPackage::from_bytes(document(None, "INSTRUCTIONS").into_bytes(), origin())
                .unwrap_err();
        assert_eq!(defaulted.code(), SkillDiagnosticCode::InvalidName);
        let empty = PreparedSkillPackage::from_bytes(
            b"---\nname: empty\ndescription: Prepared fixture.\n---\n   \n".to_vec(),
            origin(),
        )
        .unwrap_err();
        assert_eq!(empty.code(), SkillDiagnosticCode::MissingInstructions);
    }

    #[test]
    fn package_byte_limit_is_inclusive() {
        let mut at_limit = document(Some("bounded"), "INSTRUCTIONS").into_bytes();
        at_limit.resize(MAX_SKILL_FILE_BYTES, b'x');
        assert_eq!(at_limit.len(), MAX_SKILL_FILE_BYTES);
        assert!(PreparedSkillPackage::from_bytes(at_limit.clone(), origin()).is_ok());

        at_limit.push(b'x');
        let error = PreparedSkillPackage::from_bytes(at_limit, origin()).unwrap_err();
        assert_eq!(error.code(), SkillDiagnosticCode::SkillFileTooLarge);
    }

    #[test]
    fn local_directory_rejects_siblings_wrong_case_and_relative_paths() {
        let fixture = tempdir().unwrap();
        let directory = fixture.path().join("skill");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("skill.md"), document(Some("wrong"), "X")).unwrap();
        let wrong_case =
            PreparedSkillPackage::from_local_directory(&directory, "fixture").unwrap_err();
        assert_eq!(
            wrong_case.code(),
            SkillDiagnosticCode::UnexpectedPackageEntry
        );

        fs::remove_file(directory.join("skill.md")).unwrap();
        fs::write(
            directory.join(SKILL_FILE_NAME),
            document(Some("skill"), "X"),
        )
        .unwrap();
        fs::write(directory.join("reference.md"), "sibling").unwrap();
        let sibling =
            PreparedSkillPackage::from_local_directory(&directory, "fixture").unwrap_err();
        assert_eq!(sibling.code(), SkillDiagnosticCode::UnexpectedPackageEntry);

        let relative = PreparedSkillPackage::from_local_directory(Path::new("relative"), "fixture")
            .unwrap_err();
        assert_eq!(relative.code(), SkillDiagnosticCode::InvalidRoot);
    }

    #[cfg(unix)]
    #[test]
    fn local_directory_rejects_symlinked_roots_and_files() {
        use std::os::unix::fs::symlink;

        let fixture = tempdir().unwrap();
        let target = fixture.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(
            target.join(SKILL_FILE_NAME),
            document(Some("linked"), "SECRET"),
        )
        .unwrap();
        let linked_root = fixture.path().join("linked-root");
        symlink(&target, &linked_root).unwrap();
        assert_eq!(
            PreparedSkillPackage::from_local_directory(&linked_root, "fixture")
                .unwrap_err()
                .code(),
            SkillDiagnosticCode::SymlinkNotAllowed
        );

        let linked_file_root = fixture.path().join("linked-file-root");
        fs::create_dir(&linked_file_root).unwrap();
        symlink(
            target.join(SKILL_FILE_NAME),
            linked_file_root.join(SKILL_FILE_NAME),
        )
        .unwrap();
        assert_eq!(
            PreparedSkillPackage::from_local_directory(&linked_file_root, "fixture")
                .unwrap_err()
                .code(),
            SkillDiagnosticCode::SymlinkNotAllowed
        );
    }

    #[cfg(unix)]
    #[test]
    fn pinned_directory_identity_detects_same_path_replacement() {
        let fixture = tempdir().unwrap();
        let directory = fixture.path().join("selected");
        fs::create_dir(&directory).unwrap();
        let metadata = fs::symlink_metadata(&directory).unwrap();
        let canonical = directory.canonicalize().unwrap();
        let handle = open_local_directory(&directory, &metadata, &canonical).unwrap();
        let moved = fixture.path().join("moved");
        fs::rename(&directory, &moved).unwrap();
        fs::create_dir(&directory).unwrap();

        let error = verify_local_directory_binding(&handle, &directory, &canonical).unwrap_err();
        assert_eq!(error.code(), SkillDiagnosticCode::PathChangedDuringRead);
    }
}
