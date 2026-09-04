use super::*;

#[derive(Debug)]
struct SelectedArchiveEntry {
    index: usize,
    logical_path: String,
    size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveEntryScope {
    Unrelated,
    SelectedAncestor,
    SelectedRoot,
    SelectedDescendant,
}

impl ArchiveEntryScope {
    fn requires_strict_audit(self) -> bool {
        !matches!(self, Self::Unrelated)
    }
}

#[cfg(test)]
pub(super) fn extract_selected_skill(
    archive_bytes: &[u8],
    subdirectory: &GitHubSubdirectory,
) -> Result<Vec<(String, Vec<u8>)>, GitHubAcquisitionError> {
    extract_selected_skill_reader(Cursor::new(archive_bytes), subdirectory)
}

pub(in crate::skills) fn extract_selected_skill_archive(
    archive: &GitHubArchive,
    subdirectory: &GitHubSubdirectory,
) -> Result<Vec<(String, Vec<u8>)>, GitHubAcquisitionError> {
    let reader = archive.reader().map_err(|_| invalid_archive())?;
    extract_selected_skill_reader(reader, subdirectory)
}

fn extract_selected_skill_reader<R: Read + Seek>(
    reader: R,
    subdirectory: &GitHubSubdirectory,
) -> Result<Vec<(String, Vec<u8>)>, GitHubAcquisitionError> {
    let mut archive = ZipArchive::new(reader).map_err(|_| invalid_archive())?;
    if archive.is_empty() || archive.len() > MAX_GITHUB_ARCHIVE_ENTRIES {
        return Err(acquisition_error(
            GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
            format!("GitHub archive must contain 1 to {MAX_GITHUB_ARCHIVE_ENTRIES} entries."),
        ));
    }

    let mut top_root: Option<String> = None;
    let mut selected_portable_entries = BTreeMap::<String, String>::new();
    let mut selected_portable_files = BTreeMap::<String, String>::new();
    let mut selected_portable_directories = BTreeMap::<String, String>::new();
    let mut selected_archive_uncompressed_bytes = 0_u64;
    let mut selected_bytes = 0usize;
    let mut selected = Vec::new();

    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|_| invalid_archive())?;
        let path = validate_archive_entry_path(&entry)?;
        let root = path.wrapper_root();
        match &top_root {
            None => top_root = Some(root.to_string()),
            Some(expected) if expected == root => {}
            Some(_) => {
                return Err(acquisition_error(
                    GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
                    "GitHub archive contains more than one top-level root.",
                ));
            }
        }

        // The repository is untrusted, but only the selected subtree can cross the package
        // boundary. Unrelated entries still need canonical paths so membership and the codeload
        // wrapper are unambiguous; their file modes and compressed contents are never consumed.
        let scope = path.classify(subdirectory);
        if !scope.requires_strict_audit() {
            continue;
        }

        validate_strict_archive_entry(&entry)?;
        let components = path.strict_utf8_components()?;
        let repository_relative = &components[1..];
        let canonical_path = components.join("/");
        let collision_key = canonical_path.to_lowercase();
        if let Some(previous) =
            selected_portable_entries.insert(collision_key, canonical_path.clone())
        {
            let collision = if previous == canonical_path {
                "duplicate paths"
            } else {
                "paths that collide on a case-insensitive filesystem"
            };
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
                format!("GitHub archive contains {collision}."),
            ));
        }
        validate_portable_archive_tree(
            &components,
            entry.is_dir(),
            &mut selected_portable_files,
            &mut selected_portable_directories,
        )?;

        let size = entry.size();
        if size > MAX_GITHUB_ARCHIVE_ENTRY_BYTES {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "A selected GitHub archive entry exceeds the uncompressed entry limit.",
            ));
        }
        selected_archive_uncompressed_bytes = selected_archive_uncompressed_bytes
            .checked_add(size)
            .ok_or_else(|| {
                acquisition_error(
                    GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                    "Selected GitHub archive size accounting overflowed.",
                )
            })?;
        if selected_archive_uncompressed_bytes > MAX_GITHUB_ARCHIVE_UNCOMPRESSED_BYTES {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "Selected GitHub archive entries exceed the total uncompressed acquisition limit.",
            ));
        }
        if suspicious_expansion(size, entry.compressed_size()) {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "Selected GitHub archive entries contain an unsafe compression expansion ratio.",
            ));
        }

        match scope {
            ArchiveEntryScope::SelectedAncestor if !entry.is_dir() => {
                return Err(acquisition_error(
                    GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
                    "An ancestor of the selected Skill directory resolves to a file.",
                ));
            }
            ArchiveEntryScope::SelectedRoot if !entry.is_dir() => {
                return Err(acquisition_error(
                    GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
                    "The selected Skill directory resolves to a file.",
                ));
            }
            ArchiveEntryScope::SelectedAncestor | ArchiveEntryScope::SelectedRoot => continue,
            ArchiveEntryScope::SelectedDescendant => {}
            ArchiveEntryScope::Unrelated => {
                unreachable!("unrelated entries do not enter strict archive auditing")
            }
        }

        let logical_components = repository_relative
            .strip_prefix(subdirectory.components())
            .expect("selected descendants have the selected directory prefix");
        debug_assert!(!logical_components.is_empty());
        if entry.is_dir() {
            continue;
        }
        if selected.len() >= MAX_SKILL_PACKAGE_FILES {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                format!("Selected Skill contains more than {MAX_SKILL_PACKAGE_FILES} files."),
            ));
        }
        let logical_path = logical_components.join("/");
        let file_limit = if logical_path == SKILL_FILE_NAME {
            MAX_SKILL_FILE_BYTES
        } else {
            MAX_SKILL_RESOURCE_FILE_BYTES
        };
        let size = usize::try_from(size).map_err(|_| {
            acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "Selected Skill file size does not fit this platform.",
            )
        })?;
        if size > file_limit {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "A selected Skill file exceeds its package limit.",
            ));
        }
        selected_bytes = selected_bytes.checked_add(size).ok_or_else(|| {
            acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "Selected Skill size accounting overflowed.",
            )
        })?;
        if selected_bytes > MAX_SKILL_PACKAGE_BYTES {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                format!("Selected Skill exceeds {MAX_SKILL_PACKAGE_BYTES} bytes."),
            ));
        }
        selected.push(SelectedArchiveEntry {
            index,
            logical_path,
            size,
        });
    }

    if !selected
        .iter()
        .any(|entry| entry.logical_path == SKILL_FILE_NAME)
    {
        return Err(acquisition_error(
            GitHubAcquisitionErrorCode::SkillNotFound,
            "The selected GitHub directory does not contain an exact-case SKILL.md file.",
        ));
    }

    let mut files = Vec::with_capacity(selected.len());
    let mut actual_total = 0usize;
    for selected_entry in selected {
        let mut entry = archive
            .by_index(selected_entry.index)
            .map_err(|_| invalid_archive())?;
        let mut bytes = Vec::with_capacity(selected_entry.size.min(1024 * 1024));
        entry
            .by_ref()
            .take(selected_entry.size.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid_archive())?;
        if bytes.len() != selected_entry.size {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::InvalidArchive,
                "A GitHub archive entry did not match its declared uncompressed size.",
            ));
        }
        actual_total = actual_total.checked_add(bytes.len()).ok_or_else(|| {
            acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "Selected Skill size accounting overflowed.",
            )
        })?;
        if actual_total > MAX_SKILL_PACKAGE_BYTES {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "Selected Skill exceeds the package byte limit while reading.",
            ));
        }
        files.push((selected_entry.logical_path, bytes));
    }
    Ok(files)
}

#[derive(Debug)]
pub(in crate::skills) struct ValidatedArchiveEntryPath {
    wrapper_root: String,
    repository_relative: Vec<Vec<u8>>,
    repository_relative_utf8: Option<Vec<String>>,
}

impl ValidatedArchiveEntryPath {
    pub(in crate::skills) fn wrapper_root(&self) -> &str {
        &self.wrapper_root
    }

    pub(in crate::skills) fn repository_relative_utf8(&self) -> Option<&[String]> {
        self.repository_relative_utf8.as_deref()
    }

    pub(in crate::skills) fn is_within(&self, subdirectory: &GitHubSubdirectory) -> bool {
        matches!(
            self.classify(subdirectory),
            ArchiveEntryScope::SelectedRoot | ArchiveEntryScope::SelectedDescendant
        )
    }

    fn classify(&self, subdirectory: &GitHubSubdirectory) -> ArchiveEntryScope {
        let selected = subdirectory
            .components()
            .iter()
            .map(|component| component.as_bytes())
            .collect::<Vec<_>>();
        let candidate = self
            .repository_relative
            .iter()
            .map(Vec::as_slice)
            .collect::<Vec<_>>();
        if candidate == selected {
            ArchiveEntryScope::SelectedRoot
        } else if selected.starts_with(&candidate) {
            ArchiveEntryScope::SelectedAncestor
        } else if candidate.starts_with(&selected) {
            ArchiveEntryScope::SelectedDescendant
        } else {
            ArchiveEntryScope::Unrelated
        }
    }

    fn strict_utf8_components(&self) -> Result<Vec<String>, GitHubAcquisitionError> {
        let relative = self.repository_relative_utf8.as_ref().ok_or_else(|| {
            acquisition_error(
                GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
                "The selected GitHub Skill contains a non-UTF-8 path.",
            )
        })?;
        if relative
            .iter()
            .any(|component| component.contains(':') || component.chars().any(char::is_control))
        {
            return Err(unsafe_archive_path());
        }
        let mut components = Vec::with_capacity(relative.len().saturating_add(1));
        components.push(self.wrapper_root.clone());
        components.extend(relative.iter().cloned());
        Ok(components)
    }
}

pub(in crate::skills) fn validate_archive_entry_path(
    entry: &zip::read::ZipFile<'_>,
) -> Result<ValidatedArchiveEntryPath, GitHubAcquisitionError> {
    let raw_name = entry.name_raw();
    if raw_name.is_empty()
        || raw_name.len() > MAX_GITHUB_ARCHIVE_PATH_BYTES
        || raw_name.starts_with(b"/")
        || raw_name.contains(&b'\\')
        || raw_name.contains(&b'\0')
    {
        return Err(unsafe_archive_path());
    }
    let directory = entry.is_dir();
    let path = if directory {
        raw_name
            .strip_suffix(b"/")
            .ok_or_else(unsafe_archive_path)?
    } else {
        if raw_name.ends_with(b"/") {
            return Err(unsafe_archive_path());
        }
        raw_name
    };
    let components = path
        .split(|byte| *byte == b'/')
        .map(<[u8]>::to_vec)
        .collect::<Vec<_>>();
    if components.is_empty()
        || components.len() > MAX_GITHUB_ARCHIVE_PATH_COMPONENTS
        || components.iter().any(|component| {
            component.is_empty()
                || matches!(component.as_slice(), b"." | b"..")
                || component.len() > MAX_GITHUB_ARCHIVE_PATH_COMPONENT_BYTES
        })
    {
        return Err(unsafe_archive_path());
    }
    let wrapper_root = std::str::from_utf8(&components[0])
        .ok()
        .filter(|root| {
            !root.is_empty() && !root.contains(':') && !root.chars().any(char::is_control)
        })
        .ok_or_else(unsafe_archive_path)?
        .to_string();
    let repository_relative = components[1..].to_vec();
    let repository_relative_utf8 = repository_relative
        .iter()
        .map(|component| std::str::from_utf8(component).map(str::to_string))
        .collect::<Result<Vec<_>, _>>()
        .ok();
    Ok(ValidatedArchiveEntryPath {
        wrapper_root,
        repository_relative,
        repository_relative_utf8,
    })
}

fn validate_strict_archive_entry(
    entry: &zip::read::ZipFile<'_>,
) -> Result<(), GitHubAcquisitionError> {
    if entry.encrypted() || !is_supported_compression(entry.compression()) {
        return Err(acquisition_error(
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
            "GitHub archive contains an encrypted or unsupported entry.",
        ));
    }
    let directory = entry.is_dir();
    let mode_kind = entry.unix_mode().map(|mode| mode & 0o170000).unwrap_or(0);
    let valid_kind = if directory {
        matches!(mode_kind, 0 | 0o040000)
    } else {
        matches!(mode_kind, 0 | 0o100000)
    };
    if entry.is_symlink() || !valid_kind {
        return Err(acquisition_error(
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
            "GitHub archive contains a symlink or special filesystem entry.",
        ));
    }
    Ok(())
}

fn validate_portable_archive_tree(
    components: &[String],
    directory: bool,
    files: &mut BTreeMap<String, String>,
    directories: &mut BTreeMap<String, String>,
) -> Result<(), GitHubAcquisitionError> {
    let path = components.join("/");
    let path_key = path.to_lowercase();
    if directory {
        if files.contains_key(&path_key) {
            return Err(archive_tree_collision());
        }
        match directories.get(&path_key) {
            Some(existing) if existing != &path => return Err(archive_tree_collision()),
            Some(_) => {}
            None => {
                directories.insert(path_key, path.clone());
            }
        }
    } else {
        if directories.contains_key(&path_key) {
            return Err(archive_tree_collision());
        }
        match files.get(&path_key) {
            Some(existing) if existing != &path => return Err(archive_tree_collision()),
            Some(_) => {}
            None => {
                files.insert(path_key, path.clone());
            }
        }
    }

    let mut parent = String::new();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        parent = if parent.is_empty() {
            component.clone()
        } else {
            format!("{parent}/{component}")
        };
        let parent_key = parent.to_lowercase();
        if files.contains_key(&parent_key) {
            return Err(archive_tree_collision());
        }
        match directories.get(&parent_key) {
            Some(existing) if existing != &parent => return Err(archive_tree_collision()),
            Some(_) => {}
            None => {
                directories.insert(parent_key, parent.clone());
            }
        }
    }
    Ok(())
}

fn archive_tree_collision() -> GitHubAcquisitionError {
    acquisition_error(
        GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
        "GitHub archive contains colliding file and directory paths.",
    )
}

fn is_supported_compression(method: CompressionMethod) -> bool {
    matches!(
        method,
        CompressionMethod::Stored | CompressionMethod::Deflated
    )
}

fn suspicious_expansion(uncompressed: u64, compressed: u64) -> bool {
    if uncompressed <= EXPANSION_RATIO_GRACE_BYTES {
        return false;
    }
    let permitted = compressed
        .saturating_mul(MAX_GITHUB_EXPANSION_RATIO)
        .saturating_add(EXPANSION_RATIO_GRACE_BYTES);
    uncompressed > permitted
}
