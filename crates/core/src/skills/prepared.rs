//! Validated, immutable Skill package snapshots.
//!
//! Acquisition is deliberately separate from installation. Local directories
//! are the first adapter; future Git, URL, ZIP, or registry adapters must also
//! end by constructing this same fully validated byte snapshot before they may
//! call the managed-store installer.

use super::digest::package_revision;
use super::model::{
    SkillDiagnosticCode, SkillResourceDescriptor, SkillResourceIndex, SkillRevision,
    SKILL_PACKAGE_FORMAT_VERSION, SKILL_PACKAGE_FORMAT_VERSION_V2, SKILL_PACKAGE_FORMAT_VERSION_V3,
};
use super::origin::SkillPackageOrigin;
use super::package::{
    PackageManifest, PackageManifestEntry, SkillPackagePath, MAX_SKILL_PACKAGE_BYTES,
    MAX_SKILL_PACKAGE_DIRECTORIES, MAX_SKILL_PACKAGE_DIRECTORY_ENTRIES, MAX_SKILL_PACKAGE_FILES,
    MAX_SKILL_RESOURCE_FILE_BYTES,
};
use super::parser::parse_skill_document;
use super::workspace::{
    is_symlink_or_reparse, metadata_if_present, read_bounded_verified, verify_opened_file_identity,
    verify_plain_directory, BoundedReadError, ByteBudget, MAX_SKILL_FILE_BYTES,
};
use std::error::Error;
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

/// An exact package byte snapshot that has passed package-format validation.
///
/// The type has no unchecked constructor and exposes no mutable contents. Once
/// prepared, installation never reads the acquisition path again.
#[derive(Clone, PartialEq, Eq)]
pub struct PreparedSkillPackage {
    format_version: u32,
    source: Arc<str>,
    resources: Arc<[PreparedSkillResource]>,
    manifest_bytes: Option<Arc<[u8]>>,
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
            .field("format_version", &self.format_version)
            .field("revision", &self.revision)
            .field("name", &self.name)
            .field("source_bytes", &self.source.len())
            .field("resource_count", &self.resources.len())
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
            format_version: SKILL_PACKAGE_FORMAT_VERSION,
            source: validated.source,
            resources: Arc::from([]),
            manifest_bytes: None,
            revision,
            name: validated.name,
            description: validated.description,
            instructions_range: validated.instructions_range,
            origin,
        })
    }

    /// Imports an exact-case `SKILL.md` and any supported resource trees from
    /// a local directory, then captures and validates their bytes.
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
        let files = read_local_skill_directory(directory.as_ref())?;
        Self::from_files(files, origin)
    }

    /// Constructs a fully validated package snapshot from an acquisition
    /// adapter's owned file bytes. Paths are logical, forward-slash relative
    /// paths. A package containing only SKILL.md retains format-v1 identity;
    /// conventional references/assets/scripts resources select format v2,
    /// while any other safe sibling resource selects format v3.
    pub fn from_files(
        files: Vec<(String, Vec<u8>)>,
        origin: SkillPackageOrigin,
    ) -> Result<Self, SkillPackagePreparationError> {
        if files.is_empty() || files.len() > MAX_SKILL_PACKAGE_FILES {
            return Err(SkillPackagePreparationError::new(
                SkillDiagnosticCode::TooManyEntries,
                format!("Skill package must contain 1 to {MAX_SKILL_PACKAGE_FILES} files."),
            ));
        }

        let mut canonical = Vec::with_capacity(files.len());
        for (path, bytes) in files {
            let path = SkillPackagePath::parse(path)
                .map_err(|error| SkillPackagePreparationError::new(error.code, error.message))?;
            canonical.push((path, bytes));
        }
        canonical.sort_by(|left, right| left.0.cmp(&right.0));
        if canonical.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(SkillPackagePreparationError::new(
                SkillDiagnosticCode::InvalidResourcePath,
                "Skill package contains the same canonical path more than once.",
            ));
        }

        let skill_index = canonical
            .binary_search_by(|(path, _)| path.as_str().cmp(super::workspace::SKILL_FILE_NAME))
            .map_err(|_| {
                SkillPackagePreparationError::new(
                    SkillDiagnosticCode::MissingSkillFile,
                    "Skill package does not contain an exact-case SKILL.md file.",
                )
            })?;
        if canonical.len() == 1 {
            let (_, bytes) = canonical.pop().expect("one package file");
            return Self::from_bytes(bytes, origin);
        }

        let skill_bytes = canonical[skill_index].1.clone();
        let validated = validate_installable_skill_bytes(skill_bytes)
            .map_err(|error| SkillPackagePreparationError::new(error.code, error.message))?;
        let manifest = PackageManifest::new(
            canonical
                .iter()
                .map(|(path, bytes)| PackageManifestEntry::from_bytes(path.clone(), bytes))
                .collect(),
        )
        .map_err(|error| SkillPackagePreparationError::new(error.code, error.message))?;
        let revision = manifest.revision();
        let format_version = manifest.format_version();
        debug_assert!(matches!(
            format_version,
            SKILL_PACKAGE_FORMAT_VERSION_V2 | SKILL_PACKAGE_FORMAT_VERSION_V3
        ));
        let manifest_bytes = manifest
            .encode()
            .map_err(|error| SkillPackagePreparationError::new(error.code, error.message))?;

        let resources = canonical
            .into_iter()
            .filter(|(path, _)| path.as_str() != super::workspace::SKILL_FILE_NAME)
            .map(|(path, bytes)| {
                let manifest_entry = manifest
                    .files()
                    .iter()
                    .find(|entry| entry.path() == path.as_str())
                    .expect("manifest contains prepared resource");
                PreparedSkillResource {
                    descriptor: manifest_entry
                        .resource_descriptor()
                        .expect("non-entrypoint has resource kind"),
                    bytes: bytes.into(),
                }
            })
            .collect::<Vec<_>>();
        Ok(Self {
            format_version,
            source: validated.source,
            resources: resources.into(),
            manifest_bytes: Some(manifest_bytes.into()),
            revision,
            name: validated.name,
            description: validated.description,
            instructions_range: validated.instructions_range,
            origin,
        })
    }

    pub fn format_version(&self) -> u32 {
        self.format_version
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

    pub fn resource_index(&self) -> SkillResourceIndex {
        SkillResourceIndex::new(
            self.resources
                .iter()
                .map(|resource| resource.descriptor.clone())
                .collect(),
        )
    }

    pub(super) fn resources(&self) -> &[PreparedSkillResource] {
        &self.resources
    }

    pub(super) fn manifest_bytes(&self) -> Option<&[u8]> {
        self.manifest_bytes.as_deref()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct PreparedSkillResource {
    descriptor: SkillResourceDescriptor,
    bytes: Arc<[u8]>,
}

impl PreparedSkillResource {
    pub fn descriptor(&self) -> &SkillResourceDescriptor {
        &self.descriptor
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
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

fn read_local_skill_directory(
    directory: &Path,
) -> Result<Vec<(String, Vec<u8>)>, SkillPackagePreparationError> {
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

    let mut files = Vec::new();
    let mut directory_count = 0usize;
    let mut budget = ByteBudget::new(MAX_SKILL_PACKAGE_BYTES.saturating_add(1));
    collect_local_package_files(
        directory,
        &canonical_directory,
        directory,
        "",
        &mut files,
        &mut directory_count,
        &mut budget,
    )?;
    verify_plain_directory(
        directory,
        &canonical_directory,
        "The local Skill directory changed while its package files were read.",
    )
    .map_err(|issue| SkillPackagePreparationError::new(issue.code, issue.message))?;
    verify_local_directory_binding(&directory_handle, directory, &canonical_directory)?;
    Ok(files)
}

#[allow(clippy::too_many_arguments)]
fn collect_local_package_files(
    directory: &Path,
    canonical_directory: &Path,
    package_root: &Path,
    logical_directory: &str,
    files: &mut Vec<(String, Vec<u8>)>,
    directory_count: &mut usize,
    budget: &mut ByteBudget,
) -> Result<(), SkillPackagePreparationError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| preparation_io("Cannot read local Skill package directory", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| preparation_io("Cannot inspect local Skill package entry", error))?;
    if entries.len() > MAX_SKILL_PACKAGE_DIRECTORY_ENTRIES {
        return Err(SkillPackagePreparationError::new(
            SkillDiagnosticCode::TooManyEntries,
            format!(
                "A Skill package directory contains more than {MAX_SKILL_PACKAGE_DIRECTORY_ENTRIES} entries."
            ),
        ));
    }
    entries.sort_by_key(fs::DirEntry::file_name);

    for entry in entries {
        let name = entry.file_name().into_string().map_err(|_| {
            SkillPackagePreparationError::new(
                SkillDiagnosticCode::UnsupportedPathEncoding,
                "Skill package paths must use valid UTF-8 names.",
            )
        })?;
        let logical_path = if logical_directory.is_empty() {
            name.clone()
        } else {
            format!("{logical_directory}/{name}")
        };
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| preparation_io("Cannot inspect local Skill package entry", error))?;
        if is_symlink_or_reparse(&metadata) {
            return Err(SkillPackagePreparationError::new(
                SkillDiagnosticCode::SymlinkNotAllowed,
                format!("Local Skill package entry `{logical_path}` cannot be a symlink or reparse point."),
            ));
        }
        let canonical_path = path
            .canonicalize()
            .map_err(|error| preparation_io("Cannot resolve local Skill package entry", error))?;
        if !canonical_path.starts_with(canonical_directory) {
            return Err(SkillPackagePreparationError::new(
                SkillDiagnosticCode::RootEscapesWorkspace,
                format!("Local Skill package entry `{logical_path}` resolves outside its acquisition directory."),
            ));
        }

        if metadata.is_dir() {
            *directory_count = directory_count.saturating_add(1);
            if *directory_count > MAX_SKILL_PACKAGE_DIRECTORIES {
                return Err(SkillPackagePreparationError::new(
                    SkillDiagnosticCode::TooManyEntries,
                    format!("Skill package contains more than {MAX_SKILL_PACKAGE_DIRECTORIES} directories."),
                ));
            }
            SkillPackagePath::parse(format!("{logical_path}/.directory-contract"))
                .map_err(|error| SkillPackagePreparationError::new(error.code, error.message))?;
            verify_plain_directory(
                &path,
                &canonical_path,
                "A local Skill package directory changed while it was inspected.",
            )
            .map_err(|issue| SkillPackagePreparationError::new(issue.code, issue.message))?;
            collect_local_package_files(
                &path,
                canonical_directory,
                package_root,
                &logical_path,
                files,
                directory_count,
                budget,
            )?;
            verify_plain_directory(
                &path,
                &canonical_path,
                "A local Skill package directory changed while its children were read.",
            )
            .map_err(|issue| SkillPackagePreparationError::new(issue.code, issue.message))?;
            continue;
        }
        if !metadata.is_file() {
            return Err(SkillPackagePreparationError::new(
                SkillDiagnosticCode::UnexpectedPackageEntry,
                format!("Local Skill package entry `{logical_path}` is not a regular file."),
            ));
        }
        if files.len() >= MAX_SKILL_PACKAGE_FILES {
            return Err(SkillPackagePreparationError::new(
                SkillDiagnosticCode::TooManyEntries,
                format!("Skill package contains more than {MAX_SKILL_PACKAGE_FILES} files."),
            ));
        }
        SkillPackagePath::parse(logical_path.clone())
            .map_err(|error| SkillPackagePreparationError::new(error.code, error.message))?;
        let max_bytes = if logical_path == super::workspace::SKILL_FILE_NAME {
            MAX_SKILL_FILE_BYTES
        } else {
            MAX_SKILL_RESOURCE_FILE_BYTES
        };
        let bytes = read_bounded_verified(
            &path,
            &canonical_path,
            &metadata,
            canonical_directory,
            &package_root.canonicalize().map_err(|error| {
                preparation_io("Cannot resolve local Skill package root", error)
            })?,
            max_bytes,
            budget,
        )
        .map_err(|error| match error {
            BoundedReadError::TooLarge => SkillPackagePreparationError::new(
                if logical_path == super::workspace::SKILL_FILE_NAME {
                    SkillDiagnosticCode::SkillFileTooLarge
                } else {
                    SkillDiagnosticCode::ResourceFileTooLarge
                },
                format!("Skill package file `{logical_path}` exceeds {max_bytes} bytes."),
            ),
            BoundedReadError::CatalogBudgetExceeded => SkillPackagePreparationError::new(
                SkillDiagnosticCode::PackageTooLarge,
                format!("Skill package exceeds {MAX_SKILL_PACKAGE_BYTES} bytes."),
            ),
            BoundedReadError::Io(error) => {
                preparation_io("Cannot read local Skill package file", error)
            }
            BoundedReadError::PathChanged(reason) => SkillPackagePreparationError::new(
                SkillDiagnosticCode::PathChangedDuringRead,
                format!(
                    "Local Skill package file `{logical_path}` changed while it was read: {reason}"
                ),
            ),
        })?;
        files.push((logical_path, bytes));
    }
    Ok(())
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

fn preparation_io(action: &str, error: std::io::Error) -> SkillPackagePreparationError {
    SkillPackagePreparationError::new(
        SkillDiagnosticCode::UnreadableEntry,
        format!("{action}: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::workspace::SKILL_FILE_NAME;
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
    fn file_tree_with_resources_produces_a_deterministic_v2_snapshot() {
        let source = document(Some("resourceful"), "USE_REFERENCES").into_bytes();
        let first = PreparedSkillPackage::from_files(
            vec![
                ("references/guide.md".to_string(), b"GUIDE_V1".to_vec()),
                ("assets/icon.bin".to_string(), vec![0, 1, 2, 255]),
                (SKILL_FILE_NAME.to_string(), source.clone()),
            ],
            origin(),
        )
        .unwrap();
        let second = PreparedSkillPackage::from_files(
            vec![
                (SKILL_FILE_NAME.to_string(), source),
                ("assets/icon.bin".to_string(), vec![0, 1, 2, 255]),
                ("references/guide.md".to_string(), b"GUIDE_V1".to_vec()),
            ],
            origin(),
        )
        .unwrap();

        assert_eq!(first.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V2);
        assert_eq!(first.revision(), second.revision());
        assert_eq!(first.resource_index().len(), 2);
        assert_eq!(
            first
                .resource_index()
                .get("references/guide.md")
                .unwrap()
                .kind(),
            super::super::model::SkillResourceKind::Reference
        );
        assert!(first.manifest_bytes().is_some());

        let changed = PreparedSkillPackage::from_files(
            vec![
                (
                    SKILL_FILE_NAME.to_string(),
                    document(Some("resourceful"), "USE_REFERENCES").into_bytes(),
                ),
                ("references/guide.md".to_string(), b"GUIDE_V2".to_vec()),
            ],
            origin(),
        )
        .unwrap();
        assert_ne!(first.revision(), changed.revision());
    }

    #[test]
    fn local_directory_captures_nested_resources_without_retaining_the_source_path() {
        let fixture = tempdir().unwrap();
        let directory = fixture.path().join("resourceful");
        fs::create_dir_all(directory.join("references/deep")).unwrap();
        fs::create_dir(directory.join("assets")).unwrap();
        fs::write(
            directory.join(SKILL_FILE_NAME),
            document(Some("resourceful"), "READ_THE_GUIDE"),
        )
        .unwrap();
        fs::write(directory.join("references/deep/guide.md"), "ORIGINAL").unwrap();
        fs::write(directory.join("assets/data.bin"), [0, 255]).unwrap();

        let package = PreparedSkillPackage::from_local_directory(&directory, "fixture").unwrap();
        fs::write(directory.join("references/deep/guide.md"), "MUTATED").unwrap();

        assert_eq!(package.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V2);
        assert_eq!(package.resources().len(), 2);
        assert_eq!(
            package
                .resources()
                .iter()
                .find(|resource| resource.descriptor().path() == "references/deep/guide.md")
                .unwrap()
                .bytes(),
            b"ORIGINAL"
        );
    }

    #[test]
    fn generic_resources_select_v3_and_preserve_relative_paths_and_bytes() {
        let source = document(Some("portable"), "READ_ALL_RESOURCES").into_bytes();
        let package = PreparedSkillPackage::from_files(
            vec![
                (SKILL_FILE_NAME.to_string(), source),
                ("README.md".to_string(), b"ROOT_README".to_vec()),
                (
                    "agents/openai.yaml".to_string(),
                    b"interface: chat".to_vec(),
                ),
                ("references/guide.md".to_string(), b"GUIDE".to_vec()),
            ],
            origin(),
        )
        .unwrap();

        assert_eq!(package.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V3);
        assert!(package
            .revision()
            .as_str()
            .starts_with("skill-package-sha256-v3:"));
        assert_eq!(
            package.resource_index().get("README.md").unwrap().kind(),
            super::super::model::SkillResourceKind::Other
        );
        assert_eq!(
            package
                .resources()
                .iter()
                .find(|resource| resource.descriptor().path() == "agents/openai.yaml")
                .unwrap()
                .bytes(),
            b"interface: chat"
        );
        assert_eq!(
            package
                .resource_index()
                .get("references/guide.md")
                .unwrap()
                .kind(),
            super::super::model::SkillResourceKind::Reference
        );
    }

    #[test]
    fn local_directory_imports_generic_resources_as_v3() {
        let fixture = tempdir().unwrap();
        let directory = fixture.path().join("portable");
        fs::create_dir_all(directory.join("agents")).unwrap();
        fs::write(
            directory.join(SKILL_FILE_NAME),
            document(Some("portable"), "USE_AGENT_METADATA"),
        )
        .unwrap();
        fs::write(directory.join("README.md"), "README").unwrap();
        fs::write(directory.join("agents/openai.yaml"), "interface: chat").unwrap();

        let package = PreparedSkillPackage::from_local_directory(&directory, "fixture").unwrap();

        assert_eq!(package.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V3);
        assert_eq!(package.resources().len(), 2);
        assert_eq!(
            package
                .resources()
                .iter()
                .map(|resource| resource.descriptor().path())
                .collect::<Vec<_>>(),
            vec!["README.md", "agents/openai.yaml"]
        );
    }

    #[test]
    fn v2_preparation_rejects_collisions_and_resource_budget_overflow() {
        let source = document(Some("bounded-v2"), "INSTRUCTIONS").into_bytes();
        let collision = PreparedSkillPackage::from_files(
            vec![
                (SKILL_FILE_NAME.to_string(), source.clone()),
                ("references/A.md".to_string(), b"A".to_vec()),
                ("references/a.md".to_string(), b"a".to_vec()),
            ],
            origin(),
        )
        .unwrap_err();
        assert_eq!(collision.code(), SkillDiagnosticCode::InvalidResourcePath);

        let file_directory_collision = PreparedSkillPackage::from_files(
            vec![
                (SKILL_FILE_NAME.to_string(), source.clone()),
                ("references/guide".to_string(), b"file".to_vec()),
                (
                    "references/guide/section.md".to_string(),
                    b"nested".to_vec(),
                ),
            ],
            origin(),
        )
        .unwrap_err();
        assert_eq!(
            file_directory_collision.code(),
            SkillDiagnosticCode::InvalidResourcePath
        );

        let oversized = PreparedSkillPackage::from_files(
            vec![
                (SKILL_FILE_NAME.to_string(), source),
                (
                    "assets/large.bin".to_string(),
                    vec![0; MAX_SKILL_RESOURCE_FILE_BYTES + 1],
                ),
            ],
            origin(),
        )
        .unwrap_err();
        assert_eq!(oversized.code(), SkillDiagnosticCode::ResourceFileTooLarge);
    }

    #[test]
    fn v2_file_count_budget_is_inclusive() {
        let mut files = vec![(
            SKILL_FILE_NAME.to_string(),
            document(Some("many-files"), "INSTRUCTIONS").into_bytes(),
        )];
        for index in 0..MAX_SKILL_PACKAGE_FILES - 1 {
            files.push((format!("references/file-{index:04}.md"), b"x".to_vec()));
        }
        assert!(PreparedSkillPackage::from_files(files.clone(), origin()).is_ok());
        files.push(("references/overflow.md".to_string(), b"x".to_vec()));
        let error = PreparedSkillPackage::from_files(files, origin()).unwrap_err();
        assert_eq!(error.code(), SkillDiagnosticCode::TooManyEntries);
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
    fn local_directory_rejects_wrong_case_entrypoints_and_relative_paths() {
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

        let linked_resource_root = fixture.path().join("linked-resource-root");
        fs::create_dir_all(linked_resource_root.join("references")).unwrap();
        fs::write(
            linked_resource_root.join(SKILL_FILE_NAME),
            document(Some("linked-resource"), "READ_RESOURCE"),
        )
        .unwrap();
        symlink(
            target.join(SKILL_FILE_NAME),
            linked_resource_root.join("references/guide.md"),
        )
        .unwrap();
        assert_eq!(
            PreparedSkillPackage::from_local_directory(&linked_resource_root, "fixture")
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
