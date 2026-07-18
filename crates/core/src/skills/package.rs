//! Canonical logical file trees for managed Skill package formats v2 and v3.
//!
//! A package is content, not a filesystem snapshot: permissions,
//! timestamps, extended attributes, and empty directories are deliberately
//! excluded. Every acquisition adapter must produce the same canonical file
//! set before installation.

use super::digest::{package_file_digest, package_revision_v2, package_revision_v3};
use super::model::{
    SkillDiagnosticCode, SkillResourceDescriptor, SkillResourceIndex, SkillResourceKind,
    SkillRevision, SKILL_PACKAGE_FORMAT_VERSION_V2, SKILL_PACKAGE_FORMAT_VERSION_V3,
};
use super::workspace::{MAX_SKILL_FILE_BYTES, SKILL_FILE_NAME};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const PACKAGE_MANIFEST_FILE: &str = ".mycopilot-package.json";
pub(super) const PACKAGE_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub(super) const MAX_SKILL_PACKAGE_FILES: usize = 1_024;
pub(super) const MAX_SKILL_PACKAGE_DIRECTORIES: usize = 256;
pub(super) const MAX_SKILL_PACKAGE_DEPTH: usize = 16;
pub(super) const MAX_SKILL_PACKAGE_PATH_BYTES: usize = 1_024;
pub(super) const MAX_SKILL_PACKAGE_COMPONENT_BYTES: usize = 255;
pub(super) const MAX_SKILL_PACKAGE_DIRECTORY_ENTRIES: usize = 1_024;
pub(super) const MAX_SKILL_RESOURCE_FILE_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_SKILL_PACKAGE_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_SKILL_PACKAGE_MANIFEST_BYTES: usize = 2 * 1024 * 1024;

const REFERENCES_DIRECTORY: &str = "references";
const ASSETS_DIRECTORY: &str = "assets";
const SCRIPTS_DIRECTORY: &str = "scripts";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct SkillPackagePath(String);

impl SkillPackagePath {
    pub fn parse(value: impl Into<String>) -> Result<Self, PackageValidationError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_SKILL_PACKAGE_PATH_BYTES {
            return Err(package_error(
                SkillDiagnosticCode::InvalidResourcePath,
                format!(
                    "Skill package path must contain 1 to {MAX_SKILL_PACKAGE_PATH_BYTES} UTF-8 bytes."
                ),
            ));
        }
        if value.starts_with('/') || value.contains(['\\', '\0']) {
            return Err(package_error(
                SkillDiagnosticCode::InvalidResourcePath,
                "Skill package paths must be relative and use forward slashes.",
            ));
        }
        let components = value.split('/').collect::<Vec<_>>();
        if components.len() > MAX_SKILL_PACKAGE_DEPTH {
            return Err(package_error(
                SkillDiagnosticCode::InvalidResourcePath,
                format!(
                    "Skill package paths may contain at most {MAX_SKILL_PACKAGE_DEPTH} components."
                ),
            ));
        }
        for component in &components {
            validate_component(component)?;
        }

        if value != SKILL_FILE_NAME && value.eq_ignore_ascii_case(SKILL_FILE_NAME) {
            return Err(package_error(
                SkillDiagnosticCode::UnexpectedPackageEntry,
                "Skill package entrypoint must use the exact-case name SKILL.md.",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn resource_kind(&self) -> Option<SkillResourceKind> {
        let mut components = self.0.split('/');
        let root = components.next();
        components.next()?;
        match root {
            Some(REFERENCES_DIRECTORY) => Some(SkillResourceKind::Reference),
            Some(ASSETS_DIRECTORY) => Some(SkillResourceKind::Asset),
            Some(SCRIPTS_DIRECTORY) => Some(SkillResourceKind::Script),
            _ => None,
        }
    }

    fn portable_collision_key(&self) -> String {
        // Unicode paths retain their exact UTF-8 spelling in the revision.
        // Lowercasing additionally rejects names that would alias on the
        // common case-insensitive managed-store filesystems.
        self.0.to_lowercase()
    }
}

fn validate_component(component: &str) -> Result<(), PackageValidationError> {
    if component.is_empty()
        || matches!(component, "." | "..")
        || component.len() > MAX_SKILL_PACKAGE_COMPONENT_BYTES
        || component.chars().any(char::is_control)
        || component.ends_with(['.', ' '])
        || component.contains(['<', '>', '"', ':', '|', '?', '*'])
        || component.eq_ignore_ascii_case(PACKAGE_MANIFEST_FILE)
    {
        return Err(package_error(
            SkillDiagnosticCode::InvalidResourcePath,
            format!("Invalid or non-portable Skill package path component `{component}`."),
        ));
    }
    let device_stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .to_ascii_uppercase();
    if matches!(device_stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || device_stem
            .strip_prefix("COM")
            .or_else(|| device_stem.strip_prefix("LPT"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
    {
        return Err(package_error(
            SkillDiagnosticCode::InvalidResourcePath,
            format!("Skill package path component `{component}` is reserved on Windows."),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum PackageFileKind {
    Entrypoint,
    Reference,
    Asset,
    Script,
    Other,
}

impl PackageFileKind {
    fn from_path(path: &SkillPackagePath) -> Self {
        if path.as_str() == SKILL_FILE_NAME {
            return Self::Entrypoint;
        }
        match path.resource_kind() {
            None => Self::Other,
            Some(SkillResourceKind::Reference) => Self::Reference,
            Some(SkillResourceKind::Asset) => Self::Asset,
            Some(SkillResourceKind::Script) => Self::Script,
            Some(SkillResourceKind::Other) => Self::Other,
        }
    }

    fn stable_name(self) -> &'static str {
        match self {
            Self::Entrypoint => "entrypoint",
            Self::Reference => "reference",
            Self::Asset => "asset",
            Self::Script => "script",
            Self::Other => "other",
        }
    }

    fn resource_kind(self) -> Option<SkillResourceKind> {
        match self {
            Self::Entrypoint => None,
            Self::Reference => Some(SkillResourceKind::Reference),
            Self::Asset => Some(SkillResourceKind::Asset),
            Self::Script => Some(SkillResourceKind::Script),
            Self::Other => Some(SkillResourceKind::Other),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PackageManifestEntry {
    path: String,
    kind: PackageFileKind,
    byte_length: u64,
    digest: String,
}

impl PackageManifestEntry {
    pub fn from_bytes(path: SkillPackagePath, bytes: &[u8]) -> Self {
        Self {
            kind: PackageFileKind::from_path(&path),
            path: path.0,
            byte_length: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            digest: package_file_digest(bytes),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn kind_name(&self) -> &'static str {
        self.kind.stable_name()
    }

    pub fn byte_length(&self) -> u64 {
        self.byte_length
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn resource_descriptor(&self) -> Option<SkillResourceDescriptor> {
        self.kind.resource_kind().map(|kind| {
            SkillResourceDescriptor::new(
                self.path.clone(),
                kind,
                self.byte_length,
                self.digest.clone(),
            )
        })
    }

    fn from_resource_descriptor(
        descriptor: &SkillResourceDescriptor,
    ) -> Result<Self, PackageValidationError> {
        let path = SkillPackagePath::parse(descriptor.path().to_string())?;
        let kind = PackageFileKind::from_path(&path);
        if kind.resource_kind() != Some(descriptor.kind()) {
            return Err(package_error(
                SkillDiagnosticCode::InvalidPackageManifest,
                format!(
                    "Resolved resource kind does not match path `{}`.",
                    descriptor.path()
                ),
            ));
        }
        Ok(Self {
            path: path.0,
            kind,
            byte_length: descriptor.byte_length(),
            digest: descriptor.content_digest().to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PackageManifest {
    schema_version: u32,
    package_format_version: u32,
    entrypoint: String,
    files: Vec<PackageManifestEntry>,
}

impl PackageManifest {
    pub fn new(mut files: Vec<PackageManifestEntry>) -> Result<Self, PackageValidationError> {
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let package_format_version = if files
            .iter()
            .any(|entry| entry.kind == PackageFileKind::Other)
        {
            SKILL_PACKAGE_FORMAT_VERSION_V3
        } else {
            SKILL_PACKAGE_FORMAT_VERSION_V2
        };
        let manifest = Self {
            schema_version: PACKAGE_MANIFEST_SCHEMA_VERSION,
            package_format_version,
            entrypoint: SKILL_FILE_NAME.to_string(),
            files,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PackageValidationError> {
        if bytes.len() > MAX_SKILL_PACKAGE_MANIFEST_BYTES {
            return Err(package_error(
                SkillDiagnosticCode::InvalidPackageManifest,
                format!("Package manifest exceeds {MAX_SKILL_PACKAGE_MANIFEST_BYTES} bytes."),
            ));
        }
        if bytes.contains(&0) {
            return Err(package_error(
                SkillDiagnosticCode::InvalidPackageManifest,
                "Package manifest contains a NUL byte.",
            ));
        }
        let manifest: Self = serde_json::from_slice(bytes).map_err(|error| {
            package_error(
                SkillDiagnosticCode::InvalidPackageManifest,
                format!("Package manifest is not valid strict JSON: {error}"),
            )
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn from_resolved(
        source_bytes: &[u8],
        resources: &SkillResourceIndex,
    ) -> Result<Self, PackageValidationError> {
        let mut entries = Vec::with_capacity(resources.len().saturating_add(1));
        entries.push(PackageManifestEntry::from_bytes(
            SkillPackagePath::parse(SKILL_FILE_NAME.to_string())?,
            source_bytes,
        ));
        for resource in resources.entries() {
            entries.push(PackageManifestEntry::from_resource_descriptor(resource)?);
        }
        Self::new(entries)
    }

    pub fn encode(&self) -> Result<Vec<u8>, PackageValidationError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| {
            package_error(
                SkillDiagnosticCode::InvalidPackageManifest,
                format!("Cannot encode package manifest: {error}"),
            )
        })?;
        if bytes.len() > MAX_SKILL_PACKAGE_MANIFEST_BYTES {
            return Err(package_error(
                SkillDiagnosticCode::InvalidPackageManifest,
                format!("Package manifest exceeds {MAX_SKILL_PACKAGE_MANIFEST_BYTES} bytes."),
            ));
        }
        Ok(bytes)
    }

    pub fn revision(&self) -> SkillRevision {
        match self.package_format_version {
            SKILL_PACKAGE_FORMAT_VERSION_V2 => package_revision_v2(&self.files),
            SKILL_PACKAGE_FORMAT_VERSION_V3 => package_revision_v3(&self.files),
            _ => unreachable!("validated package manifest format"),
        }
    }

    pub fn format_version(&self) -> u32 {
        self.package_format_version
    }

    pub fn files(&self) -> &[PackageManifestEntry] {
        &self.files
    }

    pub fn entrypoint_entry(&self) -> &PackageManifestEntry {
        self.files
            .iter()
            .find(|entry| entry.path == SKILL_FILE_NAME)
            .expect("validated package manifest contains SKILL.md")
    }

    pub fn resource_descriptors(&self) -> Vec<SkillResourceDescriptor> {
        self.files
            .iter()
            .filter_map(PackageManifestEntry::resource_descriptor)
            .collect()
    }

    fn validate(&self) -> Result<(), PackageValidationError> {
        if self.schema_version != PACKAGE_MANIFEST_SCHEMA_VERSION
            || !matches!(
                self.package_format_version,
                SKILL_PACKAGE_FORMAT_VERSION_V2 | SKILL_PACKAGE_FORMAT_VERSION_V3
            )
            || self.entrypoint != SKILL_FILE_NAME
        {
            return Err(package_error(
                SkillDiagnosticCode::InvalidPackageManifest,
                "Package manifest declares an unsupported schema, format, or entrypoint.",
            ));
        }
        if self.files.len() < 2 || self.files.len() > MAX_SKILL_PACKAGE_FILES {
            return Err(package_error(
                SkillDiagnosticCode::TooManyEntries,
                format!(
                    "Package must contain SKILL.md and at least one resource, with at most {MAX_SKILL_PACKAGE_FILES} files."
                ),
            ));
        }

        let contains_other = self
            .files
            .iter()
            .any(|entry| entry.kind == PackageFileKind::Other);
        if (self.package_format_version == SKILL_PACKAGE_FORMAT_VERSION_V2 && contains_other)
            || (self.package_format_version == SKILL_PACKAGE_FORMAT_VERSION_V3 && !contains_other)
        {
            return Err(package_error(
                SkillDiagnosticCode::InvalidPackageManifest,
                "Package format v2 is reserved for conventional resource trees; format v3 requires at least one other resource.",
            ));
        }

        let mut previous_path: Option<&str> = None;
        let mut portable_files = BTreeMap::<String, String>::new();
        let mut portable_directories = BTreeMap::<String, String>::new();
        let mut directories = BTreeSet::new();
        let mut directory_entries = BTreeMap::<String, usize>::new();
        let mut total_bytes = 0usize;
        let mut entrypoint_count = 0usize;
        for entry in &self.files {
            let path = SkillPackagePath::parse(entry.path.clone())?;
            if previous_path.is_some_and(|previous| previous >= entry.path.as_str()) {
                return Err(package_error(
                    SkillDiagnosticCode::InvalidPackageManifest,
                    "Package manifest files must use unique canonical sorted paths.",
                ));
            }
            previous_path = Some(&entry.path);
            validate_portable_tree_path(&path, &mut portable_files, &mut portable_directories)?;
            account_path_structure(path.as_str(), &mut directories, &mut directory_entries)?;
            if entry.kind != PackageFileKind::from_path(&path) {
                return Err(package_error(
                    SkillDiagnosticCode::InvalidPackageManifest,
                    format!(
                        "Package manifest kind does not match path `{}`.",
                        entry.path
                    ),
                ));
            }
            validate_file_digest(&entry.digest)?;
            let byte_length = usize::try_from(entry.byte_length).map_err(|_| {
                package_error(
                    SkillDiagnosticCode::PackageTooLarge,
                    "Package file length does not fit this platform.",
                )
            })?;
            let limit = if entry.path == SKILL_FILE_NAME {
                entrypoint_count = entrypoint_count.saturating_add(1);
                MAX_SKILL_FILE_BYTES
            } else {
                MAX_SKILL_RESOURCE_FILE_BYTES
            };
            if byte_length > limit {
                return Err(package_error(
                    if entry.path == SKILL_FILE_NAME {
                        SkillDiagnosticCode::SkillFileTooLarge
                    } else {
                        SkillDiagnosticCode::ResourceFileTooLarge
                    },
                    format!("Package file `{}` exceeds {limit} bytes.", entry.path),
                ));
            }
            total_bytes = total_bytes.saturating_add(byte_length);
            if total_bytes > MAX_SKILL_PACKAGE_BYTES {
                return Err(package_error(
                    SkillDiagnosticCode::PackageTooLarge,
                    format!("Skill package exceeds {MAX_SKILL_PACKAGE_BYTES} bytes."),
                ));
            }
        }
        if entrypoint_count != 1 {
            return Err(package_error(
                SkillDiagnosticCode::InvalidPackageManifest,
                "Package manifest must contain exactly one SKILL.md entrypoint.",
            ));
        }
        if directories.len() > MAX_SKILL_PACKAGE_DIRECTORIES {
            return Err(package_error(
                SkillDiagnosticCode::TooManyEntries,
                format!(
                    "Skill package contains more than {MAX_SKILL_PACKAGE_DIRECTORIES} directories."
                ),
            ));
        }
        Ok(())
    }
}

fn validate_portable_tree_path(
    path: &SkillPackagePath,
    files: &mut BTreeMap<String, String>,
    directories: &mut BTreeMap<String, String>,
) -> Result<(), PackageValidationError> {
    let path_key = path.portable_collision_key();
    if files.insert(path_key.clone(), path.0.clone()).is_some() {
        return Err(package_error(
            SkillDiagnosticCode::InvalidResourcePath,
            "Package contains file paths that collide on a case-insensitive filesystem.",
        ));
    }
    if directories.contains_key(&path_key) {
        return Err(package_error(
            SkillDiagnosticCode::InvalidResourcePath,
            format!(
                "Package path `{}` is both a file and a directory.",
                path.as_str()
            ),
        ));
    }

    let components = path.as_str().split('/').collect::<Vec<_>>();
    let mut parent = String::new();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        parent = if parent.is_empty() {
            (*component).to_string()
        } else {
            format!("{parent}/{component}")
        };
        let parent_key = parent.to_lowercase();
        if files.contains_key(&parent_key) {
            return Err(package_error(
                SkillDiagnosticCode::InvalidResourcePath,
                format!("Package path `{parent}` is both a file and a directory."),
            ));
        }
        match directories.get(&parent_key) {
            Some(existing) if existing != &parent => {
                return Err(package_error(
                    SkillDiagnosticCode::InvalidResourcePath,
                    format!(
                        "Package directories `{existing}` and `{parent}` collide on a case-insensitive filesystem."
                    ),
                ));
            }
            Some(_) => {}
            None => {
                directories.insert(parent_key, parent.clone());
            }
        }
    }
    Ok(())
}

fn account_path_structure(
    path: &str,
    directories: &mut BTreeSet<String>,
    directory_entries: &mut BTreeMap<String, usize>,
) -> Result<(), PackageValidationError> {
    let components = path.split('/').collect::<Vec<_>>();
    let mut parent = String::new();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        let directory = if parent.is_empty() {
            (*component).to_string()
        } else {
            format!("{parent}/{component}")
        };
        if directories.insert(directory.clone()) {
            let count = directory_entries.entry(parent.clone()).or_default();
            *count = count.saturating_add(1);
        }
        parent = directory;
    }
    let count = directory_entries.entry(parent).or_default();
    *count = count.saturating_add(1);
    if directory_entries
        .values()
        .any(|count| *count > MAX_SKILL_PACKAGE_DIRECTORY_ENTRIES)
    {
        return Err(package_error(
            SkillDiagnosticCode::TooManyEntries,
            format!(
                "A Skill package directory contains more than {MAX_SKILL_PACKAGE_DIRECTORY_ENTRIES} entries."
            ),
        ));
    }
    Ok(())
}

pub(super) fn validate_file_digest(value: &str) -> Result<(), PackageValidationError> {
    const PREFIX: &str = "skill-file-sha256-v1:";
    let Some(digest) = value.strip_prefix(PREFIX) else {
        return Err(package_error(
            SkillDiagnosticCode::InvalidPackageManifest,
            format!("Package file digest must start with `{PREFIX}`."),
        ));
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(package_error(
            SkillDiagnosticCode::InvalidPackageManifest,
            "Package file digest must contain exactly 64 lowercase hexadecimal characters.",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageValidationError {
    pub code: SkillDiagnosticCode,
    pub message: String,
}

fn package_error(code: SkillDiagnosticCode, message: impl Into<String>) -> PackageValidationError {
    PackageValidationError {
        code,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, contents: &[u8]) -> PackageManifestEntry {
        PackageManifestEntry::from_bytes(SkillPackagePath::parse(path).unwrap(), contents)
    }

    #[test]
    fn canonical_paths_accept_safe_generic_resources_without_remapping() {
        assert!(SkillPackagePath::parse("SKILL.md").is_ok());
        assert!(SkillPackagePath::parse("references/deep/guide.md").is_ok());
        assert!(SkillPackagePath::parse("assets/icon.png").is_ok());
        assert!(SkillPackagePath::parse("scripts/check.sh").is_ok());
        assert!(SkillPackagePath::parse("README.md").is_ok());
        assert!(SkillPackagePath::parse("agents/openai.yaml").is_ok());
        assert!(SkillPackagePath::parse("templates/report.md").is_ok());
        assert_eq!(
            PackageFileKind::from_path(
                &SkillPackagePath::parse("references/deep/guide.md").unwrap()
            ),
            PackageFileKind::Reference
        );
        assert_eq!(
            PackageFileKind::from_path(&SkillPackagePath::parse("README.md").unwrap()),
            PackageFileKind::Other
        );
        assert_eq!(
            PackageFileKind::from_path(
                &SkillPackagePath::parse("References/deep/guide.md").unwrap()
            ),
            PackageFileKind::Other
        );
        for invalid in [
            "skill.md",
            "assets/report?.md",
            "templates/<draft>.md",
            "agents/openai|legacy.yaml",
            "references/../secret",
            "references\\secret",
            "/references/secret",
            "assets/CON.txt",
            "scripts/trailing. ",
        ] {
            assert!(
                SkillPackagePath::parse(invalid).is_err(),
                "unexpectedly accepted {invalid}"
            );
        }
    }

    #[test]
    fn manifest_is_sorted_revision_bound_and_strictly_decoded() {
        let manifest = PackageManifest::new(vec![
            entry("references/z.md", b"Z"),
            entry("SKILL.md", b"skill"),
            entry("assets/a.bin", b"A"),
        ])
        .unwrap();
        assert_eq!(
            manifest
                .files()
                .iter()
                .map(PackageManifestEntry::path)
                .collect::<Vec<_>>(),
            vec!["SKILL.md", "assets/a.bin", "references/z.md"]
        );
        let bytes = manifest.encode().unwrap();
        assert_eq!(PackageManifest::decode(&bytes).unwrap(), manifest);
        assert_eq!(manifest.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V2);

        let mut changed = manifest.clone();
        changed.files[1].digest = package_file_digest(b"changed");
        assert_ne!(manifest.revision(), changed.revision());

        let mut value = serde_json::to_value(&manifest).unwrap();
        value["unknown"] = serde_json::json!(true);
        assert!(PackageManifest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn v3_manifest_roundtrips_generic_resources_and_preserves_v2_identity() {
        let v2 = PackageManifest::new(vec![
            entry("SKILL.md", b"skill"),
            entry("references/guide.md", b"guide"),
        ])
        .unwrap();
        let v3 = PackageManifest::new(vec![
            entry("SKILL.md", b"skill"),
            entry("references/guide.md", b"guide"),
            entry("README.md", b"readme"),
            entry("agents/openai.yaml", b"interface: chat"),
        ])
        .unwrap();

        assert_eq!(v2.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V2);
        assert_eq!(v3.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V3);
        assert!(v2
            .revision()
            .as_str()
            .starts_with("skill-package-sha256-v2:"));
        assert!(v3
            .revision()
            .as_str()
            .starts_with("skill-package-sha256-v3:"));
        assert_eq!(PackageManifest::decode(&v3.encode().unwrap()).unwrap(), v3);
        assert_eq!(
            v3.resource_descriptors()
                .iter()
                .find(|resource| resource.path() == "agents/openai.yaml")
                .unwrap()
                .kind(),
            SkillResourceKind::Other
        );

        let mut falsely_v2 = serde_json::to_value(&v3).unwrap();
        falsely_v2["packageFormatVersion"] = serde_json::json!(2);
        assert!(PackageManifest::decode(&serde_json::to_vec(&falsely_v2).unwrap()).is_err());

        let mut falsely_v3 = serde_json::to_value(&v2).unwrap();
        falsely_v3["packageFormatVersion"] = serde_json::json!(3);
        assert!(PackageManifest::decode(&serde_json::to_vec(&falsely_v3).unwrap()).is_err());
    }

    #[test]
    fn manifest_rejects_case_collisions_and_noncanonical_order() {
        let collision = PackageManifest::new(vec![
            entry("SKILL.md", b"skill"),
            entry("references/A.md", b"A"),
            entry("references/a.md", b"a"),
        ])
        .unwrap_err();
        assert_eq!(collision.code, SkillDiagnosticCode::InvalidResourcePath);

        let manifest = PackageManifest::new(vec![
            entry("SKILL.md", b"skill"),
            entry("references/a.md", b"A"),
        ])
        .unwrap();
        let mut value = serde_json::to_value(manifest).unwrap();
        value["files"].as_array_mut().unwrap().reverse();
        assert!(PackageManifest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn manifest_rejects_file_directory_and_directory_case_collisions() {
        for entries in [
            vec![
                entry("SKILL.md", b"skill"),
                entry("references/a", b"file"),
                entry("references/a/guide.md", b"nested"),
            ],
            vec![
                entry("SKILL.md", b"skill"),
                entry("references/A/first.md", b"first"),
                entry("references/a/second.md", b"second"),
            ],
        ] {
            let error = PackageManifest::new(entries).unwrap_err();
            assert_eq!(error.code, SkillDiagnosticCode::InvalidResourcePath);
        }

        let valid = PackageManifest::new(vec![
            entry("SKILL.md", b"skill"),
            entry("references/a", b"file"),
            entry("references/b/guide.md", b"nested"),
        ])
        .unwrap();
        let mut encoded = serde_json::to_value(valid).unwrap();
        encoded["files"][2]["path"] = serde_json::json!("references/a/guide.md");
        let error = PackageManifest::decode(&serde_json::to_vec(&encoded).unwrap()).unwrap_err();
        assert_eq!(error.code, SkillDiagnosticCode::InvalidResourcePath);
    }

    #[test]
    fn manifest_enforces_the_directory_budget_for_non_filesystem_adapters() {
        let mut entries = vec![entry("SKILL.md", b"skill")];
        for index in 0..MAX_SKILL_PACKAGE_DIRECTORIES - 1 {
            entries.push(entry(&format!("references/d{index:03}/guide.md"), b"guide"));
        }
        assert!(PackageManifest::new(entries.clone()).is_ok());
        entries.push(entry(
            &format!("references/d{:03}/guide.md", MAX_SKILL_PACKAGE_DIRECTORIES),
            b"guide",
        ));
        let error = PackageManifest::new(entries).unwrap_err();
        assert_eq!(error.code, SkillDiagnosticCode::TooManyEntries);
    }
}
