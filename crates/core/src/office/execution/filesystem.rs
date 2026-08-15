use super::*;

pub(super) fn canonical_workspace(workspace: &Path) -> Result<PathBuf, OfficeEngineError> {
    let canonical = workspace.canonicalize().map_err(|error| {
        workspace_error(format!(
            "Cannot resolve workspace `{}`: {error}",
            workspace.display()
        ))
    })?;
    if !canonical.is_dir() {
        return Err(workspace_error(
            "The selected workspace is not a directory.",
        ));
    }
    Ok(canonical)
}

fn clean_workspace_relative_path(value: &str) -> Result<PathBuf, OfficeEngineError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid_request("Relative Office path cannot be empty."));
    }
    let mut output = PathBuf::new();
    for component in Path::new(value).components() {
        match component {
            Component::Normal(value) => output.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(workspace_error(
                    "Relative Office paths cannot contain `..`, a root, or a platform prefix.",
                ));
            }
        }
    }
    if output.as_os_str().is_empty() {
        Err(invalid_request("Relative Office path cannot be empty."))
    } else {
        Ok(output)
    }
}

pub(super) fn freeze_path(
    context: &ResolvedExecutionContext,
    slot: OfficePathSlot,
    logical_path: &str,
    purpose: OfficePathPurpose,
) -> Result<OfficeFrozenPath, OfficeEngineError> {
    let logical_path = logical_path.trim();
    if logical_path.is_empty() {
        return Err(invalid_request("Office path cannot be empty."));
    }
    if logical_path.contains('\0') {
        return Err(invalid_request("Office paths cannot contain NUL bytes."));
    }

    let (candidate, attachment) = if logical_path.starts_with("@attachments/") {
        if purpose != OfficePathPurpose::ReadSource {
            return Err(workspace_error(
                "Registered attachments are immutable read sources and cannot be Office write targets.",
            ));
        }
        (resolve_attachment(context, logical_path)?, true)
    } else if let Some(expanded) = expand_system_path(logical_path).map_err(workspace_error)? {
        (normalize_absolute_path(&expanded)?, false)
    } else {
        let path = Path::new(logical_path);
        if path.is_absolute() {
            (normalize_absolute_path(path)?, false)
        } else {
            let workspace = context.workspace.as_deref().ok_or_else(|| {
                workspace_error("Relative Office paths require a selected workspace.")
            })?;
            (
                workspace.join(clean_workspace_relative_path(logical_path)?),
                false,
            )
        }
    };

    let lexical_scope = if attachment {
        OfficePathScope::Attachment
    } else if context
        .workspace
        .as_deref()
        .is_some_and(|workspace| candidate.starts_with(workspace))
    {
        OfficePathScope::Workspace
    } else {
        OfficePathScope::External
    };
    authorize_path(context.permissions, purpose, lexical_scope)?;

    let requires_existing = purpose != OfficePathPurpose::WriteTarget;
    reject_symlink_components(&candidate, !requires_existing)?;
    let metadata = match fs::symlink_metadata(&candidate) {
        Ok(metadata) => Some(metadata),
        Err(error) if !requires_existing && error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(workspace_error(format!(
                "Cannot inspect Office path `{}`: {error}",
                candidate.display()
            )))
        }
    };

    let normalized = if metadata.is_some() {
        candidate.canonicalize().map_err(|error| {
            workspace_error(format!(
                "Cannot canonicalize Office path `{}`: {error}",
                candidate.display()
            ))
        })?
    } else {
        let parent = candidate
            .parent()
            .ok_or_else(|| workspace_error("Office output path has no parent directory."))?;
        let parent = parent.canonicalize().map_err(|error| {
            workspace_error(format!(
                "Office output parent `{}` must already exist and be accessible: {error}",
                parent.display()
            ))
        })?;
        parent.join(
            candidate
                .file_name()
                .ok_or_else(|| workspace_error("Office output path has no file name."))?,
        )
    };
    let scope = if attachment {
        OfficePathScope::Attachment
    } else if context
        .workspace
        .as_deref()
        .is_some_and(|workspace| normalized.starts_with(workspace))
    {
        OfficePathScope::Workspace
    } else {
        OfficePathScope::External
    };
    if scope != lexical_scope {
        return Err(workspace_error(
            "Office path scope changed during normalization; a symlink or mount escape may be present.",
        ));
    }
    authorize_path(context.permissions, purpose, scope)?;

    let parent = normalized
        .parent()
        .ok_or_else(|| workspace_error("Office path has no parent directory."))?;
    reject_symlink_components(parent, false)?;
    let parent_metadata = fs::symlink_metadata(parent).map_err(|error| {
        workspace_error(format!(
            "Cannot inspect Office parent `{}`: {error}",
            parent.display()
        ))
    })?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err(workspace_error(
            "Office path parent must be a real directory, not a symlink or special file.",
        ));
    }
    let parent_identity = path_identity(parent, &parent_metadata)?;

    let (state, object_identity, content_revision, size) = match metadata {
        Some(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(workspace_error(format!(
                    "Office path `{}` must be a regular non-symlink file.",
                    normalized.display()
                )));
            }
            let identity = path_identity(&normalized, &metadata)?;
            let (revision, size) = file_revision(&normalized)?;
            (
                OfficeFileState::Present,
                Some(identity),
                Some(revision),
                Some(size),
            )
        }
        None => (OfficeFileState::Missing, None, None, None),
    };
    if requires_existing && state != OfficeFileState::Present {
        return Err(workspace_error(
            "Office source or in-place target must exist.",
        ));
    }
    let write_disposition = match purpose {
        OfficePathPurpose::ReadSource => None,
        OfficePathPurpose::WriteTarget | OfficePathPurpose::InPlaceTarget => Some(match state {
            OfficeFileState::Missing => OfficeWriteDisposition::CreateNew,
            OfficeFileState::Present => OfficeWriteDisposition::ReplaceExisting,
        }),
    };

    Ok(OfficeFrozenPath {
        slot,
        logical_path: logical_path.to_string(),
        purpose,
        scope,
        normalized_path: normalized.to_string_lossy().into_owned(),
        state,
        object_identity,
        parent_identity,
        content_revision,
        size,
        write_disposition,
    })
}

fn authorize_path(
    permissions: AgentPermissions,
    purpose: OfficePathPurpose,
    scope: OfficePathScope,
) -> Result<(), OfficeEngineError> {
    match purpose {
        OfficePathPurpose::ReadSource => {
            if scope == OfficePathScope::External && permissions.read != AgentReadPermission::All {
                return Err(workspace_error(
                    "Reading an Office source outside the workspace requires read=all.",
                ));
            }
        }
        OfficePathPurpose::WriteTarget | OfficePathPurpose::InPlaceTarget => {
            if permissions.write == AgentWritePermission::Denied {
                return Err(workspace_error(
                    "Office file changes are disabled by write=denied.",
                ));
            }
            if scope != OfficePathScope::Workspace && permissions.write != AgentWritePermission::All
            {
                return Err(workspace_error(
                    "Writing an Office target outside the workspace requires write=all.",
                ));
            }
        }
    }
    Ok(())
}

fn resolve_attachment(
    context: &ResolvedExecutionContext,
    logical_path: &str,
) -> Result<PathBuf, OfficeEngineError> {
    let library = context.attachment_library.as_ref().ok_or_else(|| {
        workspace_error("No registered attachment library is available for this Office run.")
    })?;
    let attachment_id = logical_path
        .strip_prefix("@attachments/")
        .and_then(|remainder| remainder.split('/').next())
        .filter(|id| !id.is_empty())
        .ok_or_else(|| invalid_request("Attachment paths must include an attachment id."))?;
    let reference = library
        .conversation_attachments
        .iter()
        .chain(library.project_attachments.iter())
        .find(|reference| reference.id == attachment_id)
        .ok_or_else(|| {
            workspace_error(format!(
                "Registered attachment `{attachment_id}` was not found."
            ))
        })?;
    if reference.read_path != logical_path {
        return Err(workspace_error(
            "Office attachment paths must exactly match the registered readPath.",
        ));
    }
    let root = library
        .root_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| workspace_error("The registered attachment library has no root path."))?;
    let root = Path::new(root).canonicalize().map_err(|error| {
        workspace_error(format!(
            "Cannot resolve the attachment library root: {error}"
        ))
    })?;
    if !root.is_dir() {
        return Err(workspace_error(
            "The registered attachment library root is not a directory.",
        ));
    }
    let relative = clean_workspace_relative_path(&reference.storage_rel_path)?;
    let candidate = root.join(relative);
    reject_symlink_components(&candidate, false)?;
    let canonical = candidate.canonicalize().map_err(|error| {
        workspace_error(format!(
            "Cannot resolve registered Office attachment: {error}"
        ))
    })?;
    if !canonical.starts_with(&root) {
        return Err(workspace_error(
            "Registered Office attachment escaped its attachment library.",
        ));
    }
    Ok(canonical)
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf, OfficeEngineError> {
    if !path.is_absolute() {
        return Err(invalid_request("Expected an absolute Office path."));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(workspace_error(
                    "Absolute Office paths cannot contain `..`.",
                ))
            }
        }
    }
    Ok(normalized)
}

fn reject_symlink_components(
    path: &Path,
    allow_missing_final: bool,
) -> Result<(), OfficeEngineError> {
    let mut current = PathBuf::new();
    let component_count = path.components().count();
    for (index, component) in path.components().enumerate() {
        current.push(component.as_os_str());
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(workspace_error(format!(
                        "Office path component `{}` is a symbolic link.",
                        current.display()
                    )));
                }
                if index + 1 < component_count && !metadata.is_dir() {
                    return Err(workspace_error(format!(
                        "Office path component `{}` is not a directory.",
                        current.display()
                    )));
                }
            }
            Err(error)
                if allow_missing_final
                    && index + 1 == component_count
                    && error.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(())
            }
            Err(error) => {
                return Err(workspace_error(format!(
                    "Cannot inspect Office path component `{}`: {error}",
                    current.display()
                )))
            }
        }
    }
    Ok(())
}

pub(super) fn path_identity(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<OfficePathIdentity, OfficeEngineError> {
    let mut digest = Sha256::new();
    digest.update(b"mycopilot.office.path-identity\0");
    digest.update(path.as_os_str().as_encoded_bytes());
    digest.update(if metadata.is_dir() {
        &b"directory"[..]
    } else {
        &b"file"[..]
    });
    let created = metadata
        .created()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    digest.update(created.to_le_bytes());
    #[cfg(unix)]
    let (device, inode) = {
        use std::os::unix::fs::MetadataExt;
        (Some(metadata.dev()), Some(metadata.ino()))
    };
    #[cfg(not(unix))]
    let (device, inode) = (None, None);
    if let Some(device) = device {
        digest.update(device.to_le_bytes());
    }
    if let Some(inode) = inode {
        digest.update(inode.to_le_bytes());
    }
    Ok(OfficePathIdentity {
        revision: format!("{PATH_IDENTITY_PREFIX}{}", hex_lower(&digest.finalize())),
        device,
        inode,
    })
}

pub(super) fn copy_file_snapshot(source: &Path, target: &Path) -> Result<(), OfficeEngineError> {
    let mut source_options = fs::OpenOptions::new();
    source_options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        source_options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut source_file = source_options
        .open(source)
        .map_err(|error| io_error("open the Office source snapshot", error))?;
    let before = source_file
        .metadata()
        .map_err(|error| io_error("inspect the Office source snapshot", error))?;
    if !before.is_file() || before.len() > MAX_OFFICE_DOCUMENT_BYTES {
        return Err(workspace_error(format!(
            "Office file exceeds the {MAX_OFFICE_DOCUMENT_BYTES}-byte limit."
        )));
    }
    let mut target_options = fs::OpenOptions::new();
    target_options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        target_options.mode(0o600);
    }
    let mut target_file = target_options
        .open(target)
        .map_err(|error| io_error("create the Office file snapshot", error))?;
    let copied = {
        let mut limited = (&mut source_file).take(MAX_OFFICE_DOCUMENT_BYTES + 1);
        std::io::copy(&mut limited, &mut target_file)
            .map_err(|error| io_error("copy the Office file into staging", error))?
    };
    if copied != before.len() || copied > MAX_OFFICE_DOCUMENT_BYTES {
        return Err(precondition_error(
            "Office file changed or exceeded its size limit while being copied into staging.",
        ));
    }
    let after = source_file
        .metadata()
        .map_err(|error| io_error("reinspect the Office source snapshot", error))?;
    if after.len() != before.len() {
        return Err(precondition_error(
            "Office file changed while being copied into staging.",
        ));
    }
    target_file
        .flush()
        .and_then(|_| target_file.sync_all())
        .map_err(|error| io_error("sync the staged Office file", error))?;
    fs::set_permissions(target, before.permissions())
        .map_err(|error| io_error("preserve Office file permissions in staging", error))
}

/// Makes only the Host-private mutation candidate writable by its owner. Agent input snapshots
/// remain read-only, and publication still applies the destination's frozen permissions in
/// [`StagingArea::publish`].
pub(super) fn make_private_staging_owner_writable(path: &Path) -> Result<(), OfficeEngineError> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| io_error("inspect private Office staging permissions", error))?
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o600);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)
        .map_err(|error| io_error("make private Office staging owner-writable", error))
}

pub(super) fn snapshot_prepared_resources(
    prepared: &OfficePreparedExecution,
) -> Result<(tempfile::TempDir, HashMap<String, PathBuf>), OfficeEngineError> {
    let snapshots = tempfile::Builder::new()
        .prefix("mycopilot-office-resources-")
        .tempdir()
        .map_err(|error| io_error("create private Office resource snapshots", error))?;
    let resources = prepared
        .paths
        .iter()
        .filter(|path| matches!(path.slot, OfficePathSlot::Resource { .. }))
        .collect::<Vec<_>>();
    let mut resolved = HashMap::with_capacity(resources.len());
    for (index, resource) in resources.into_iter().enumerate() {
        if resource.state != OfficeFileState::Present {
            return Err(precondition_error(
                "Office resource preconditions must describe existing files.",
            ));
        }
        let source = Path::new(&resource.normalized_path);
        let name = source
            .file_name()
            .ok_or_else(|| invalid_request("Office resource path has no file name."))?
            .to_string_lossy()
            .into_owned();
        let snapshot = snapshots.path().join(format!("{index}-{name}"));
        copy_file_snapshot(source, &snapshot)?;
        let (revision, size) = file_revision(&snapshot)?;
        if resource.content_revision.as_deref() != Some(&revision) || resource.size != Some(size) {
            return Err(precondition_error(format!(
                "Office resource `{}` changed while its private snapshot was created.",
                resource.logical_path
            )));
        }
        resolved.insert(resource.logical_path.clone(), snapshot);
    }
    Ok((snapshots, resolved))
}

pub(super) fn extend_agent_input_resource_paths(
    prepared: &OfficePreparedExecution,
    inputs: Option<&PreparedAgentFileInputs>,
    resolved: &mut HashMap<String, PathBuf>,
) -> Result<(), OfficeEngineError> {
    match (prepared.input_bindings.is_empty(), inputs) {
        (true, None) => return Ok(()),
        (true, Some(_)) | (false, None) => {
            return Err(precondition_error(
                "The private Office Agent input view does not match the frozen bindings.",
            ))
        }
        (false, Some(_)) => {}
    }
    let inputs = inputs.expect("non-empty frozen bindings require materialized inputs");
    if inputs.evidence().len() != prepared.input_bindings.len() {
        return Err(precondition_error(
            "The private Office Agent input view is incomplete.",
        ));
    }
    for binding in &prepared.input_bindings {
        let placeholder = office_agent_input_placeholder(&binding.mount_path);
        let path = inputs.root().join(&binding.mount_path);
        if resolved.insert(placeholder.clone(), path).is_some() {
            return Err(precondition_error(format!(
                "Office input placeholder `{placeholder}` collides with another frozen resource.",
            )));
        }
    }
    Ok(())
}

pub(super) fn frozen_path<'a>(
    prepared: &'a OfficePreparedExecution,
    slot: &OfficePathSlot,
) -> Option<&'a OfficeFrozenPath> {
    prepared.paths.iter().find(|path| &path.slot == slot)
}

pub(super) fn rewrite_path_bearing_properties(
    resource_paths: &HashMap<String, PathBuf>,
    argv: &mut [String],
) -> Result<(), OfficeEngineError> {
    let mut index = 0;
    while index < argv.len() {
        if argv[index] == "--prop" {
            let property_index = index + 1;
            let (name, value) = {
                let property = argv
                    .get(property_index)
                    .ok_or_else(|| invalid_request("Office `--prop` requires a value."))?;
                let (name, value) = property.split_once('=').ok_or_else(|| {
                    invalid_request("Office properties must use key=value syntax.")
                })?;
                (name.to_string(), value.to_string())
            };
            if let Some(resource) = property_resource_reference(&name, &value)? {
                let resolved = resource_paths.get(resource.path()).ok_or_else(|| {
                    precondition_error(format!(
                        "Office resource `{}` is missing from the frozen execution plan.",
                        resource.path()
                    ))
                })?;
                argv[property_index] = resource.rewrite(&name, resolved);
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(())
}

pub(super) fn validate_document_artifact(
    path: &Path,
    kind: OfficeDocumentKind,
) -> Result<(), OfficeEngineError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        invalid_output(format!(
            "OfficeCLI did not produce a readable document: {error}"
        ))
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_OFFICE_DOCUMENT_BYTES
    {
        return Err(invalid_output(format!(
            "OfficeCLI output must be a non-empty regular file no larger than {MAX_OFFICE_DOCUMENT_BYTES} bytes."
        )));
    }
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("csv"))
    {
        return Ok(());
    }

    let file = fs::File::open(path)
        .map_err(|error| invalid_output(format!("Cannot open OfficeCLI output: {error}")))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| {
        invalid_output(format!(
            "OfficeCLI output is not a valid OOXML ZIP package: {error}"
        ))
    })?;
    if archive.len() > 65_535 || archive.by_name("[Content_Types].xml").is_err() {
        return Err(invalid_output(
            "OfficeCLI output is missing required OOXML package metadata.",
        ));
    }
    let main_part = match kind {
        OfficeDocumentKind::Document => "word/document.xml",
        OfficeDocumentKind::Spreadsheet => "xl/workbook.xml",
        OfficeDocumentKind::Presentation => "ppt/presentation.xml",
    };
    if archive.by_name(main_part).is_err() {
        return Err(invalid_output(format!(
            "OfficeCLI output is missing required OOXML part `{main_part}`."
        )));
    }
    Ok(())
}

pub(super) fn validate_render_artifact(
    path: &Path,
    mode: Option<&str>,
) -> Result<(), OfficeEngineError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        invalid_output(format!(
            "OfficeCLI did not produce the requested render output: {error}"
        ))
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_OFFICE_DOCUMENT_BYTES
    {
        return Err(invalid_output(
            "OfficeCLI render output must be a non-empty bounded regular file.",
        ));
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    match mode {
        Some("html") if matches!(extension.as_deref(), Some("html" | "htm")) => Ok(()),
        Some("svg") if extension.as_deref() == Some("svg") => Ok(()),
        Some("screenshot") if extension.as_deref() == Some("png") => {
            let mut file = fs::File::open(path)
                .map_err(|error| invalid_output(format!("Cannot inspect PNG output: {error}")))?;
            let mut magic = [0_u8; 8];
            file.read_exact(&mut magic)
                .map_err(|error| invalid_output(format!("Cannot inspect PNG output: {error}")))?;
            if magic == [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a] {
                Ok(())
            } else {
                Err(invalid_output(
                    "OfficeCLI screenshot output is not a PNG file.",
                ))
            }
        }
        _ => Err(invalid_output(
            "OfficeCLI render output extension does not match the requested mode.",
        )),
    }
}

pub(super) struct StagingArea {
    directory: PathBuf,
    path: PathBuf,
    published: bool,
}

pub(super) fn office_target_commit_lock(target: &Path) -> Arc<Mutex<()>> {
    let registry = OFFICE_COMMIT_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = registry.lock().unwrap_or_else(|error| error.into_inner());
    registry.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = registry.get(target).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    registry.insert(target.to_path_buf(), Arc::downgrade(&lock));
    lock
}

impl StagingArea {
    pub(super) fn new(target: &Path) -> Result<Self, OfficeEngineError> {
        let parent = target
            .parent()
            .ok_or_else(|| workspace_error("Office target has no parent directory."))?;
        let file_name = target
            .file_name()
            .ok_or_else(|| workspace_error("Office target has no file name."))?;
        for _ in 0..16 {
            let directory = parent.join(format!(
                ".mycopilot-office-{}",
                uuid::Uuid::new_v4().simple()
            ));
            match fs::create_dir(&directory) {
                Ok(()) => {
                    set_private_directory_permissions(&directory)?;
                    let path = directory.join(file_name);
                    return Ok(Self {
                        directory,
                        path,
                        published: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(io_error("create same-directory Office staging", error));
                }
            }
        }
        Err(io_error(
            "create same-directory Office staging",
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "could not allocate a unique staging directory",
            ),
        ))
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(super) fn publish(
        &mut self,
        target: &Path,
        expected_state: OfficeFileState,
    ) -> Result<(), OfficeEngineError> {
        set_publish_permissions(&self.path, target, expected_state)?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.path)
            .map_err(|error| io_error("open staged Office output for sync", error))?;
        file.sync_all()
            .map_err(|error| io_error("sync staged Office output", error))?;
        let rename = match expected_state {
            OfficeFileState::Missing => atomic_rename_noreplace(&self.path, target),
            OfficeFileState::Present => atomic_replace(&self.path, target),
        };
        rename.map_err(|error| {
            precondition_error(format!(
                "Cannot atomically publish the Office output; target state may have changed: {error}"
            ))
        })?;
        self.published = true;
        let parent = target
            .parent()
            .ok_or_else(|| workspace_error("Published Office output has no parent directory."))?;
        sync_directory(parent).map_err(|error| {
            OfficeEngineError::new(
                OfficeEngineErrorCode::CommitIndeterminate,
                OfficeEngineRecovery::InspectState,
                format!(
                    "Office output was renamed into place but its directory could not be synced: {error}"
                ),
            )
        })?;
        Ok(())
    }
}

fn set_publish_permissions(
    staging: &Path,
    target: &Path,
    expected_state: OfficeFileState,
) -> Result<(), OfficeEngineError> {
    if expected_state == OfficeFileState::Present {
        let permissions = fs::metadata(target)
            .map_err(|error| {
                precondition_error(format!("Cannot inspect target permissions: {error}"))
            })?
            .permissions();
        return fs::set_permissions(staging, permissions)
            .map_err(|error| io_error("preserve target Office file permissions", error));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(staging, fs::Permissions::from_mode(0o600))
            .map_err(|error| io_error("protect new Office output", error))?;
    }
    Ok(())
}

impl Drop for StagingArea {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.path);
        }
        let _ = fs::remove_dir(&self.directory);
    }
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), OfficeEngineError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_error("protect same-directory Office staging", error))
}

#[cfg(windows)]
fn set_private_directory_permissions(_path: &Path) -> Result<(), OfficeEngineError> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

#[cfg(target_vendor = "apple")]
fn atomic_rename_noreplace(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "source contains NUL")
    })?;
    let target = CString::new(target.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "target contains NUL")
    })?;
    // SAFETY: both C strings are NUL-terminated and valid for the duration of the call.
    let result = unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn atomic_rename_noreplace(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "source contains NUL")
    })?;
    let target = CString::new(target.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "target contains NUL")
    })?;
    // SAFETY: pointers address live NUL-terminated C strings; renameat2 does not retain them.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(
    unix,
    not(target_vendor = "apple"),
    not(any(target_os = "linux", target_os = "android"))
))]
fn atomic_rename_noreplace(_source: &Path, _target: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this platform",
    ))
}

#[cfg(windows)]
fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    move_file(source, target, true)
}

#[cfg(windows)]
fn atomic_rename_noreplace(source: &Path, target: &Path) -> std::io::Result<()> {
    move_file(source, target, false)
}

#[cfg(windows)]
fn move_file(source: &Path, target: &Path, replace: bool) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut flags = MOVEFILE_WRITE_THROUGH;
    if replace {
        flags |= MOVEFILE_REPLACE_EXISTING;
    }
    // SAFETY: both UTF-16 buffers are NUL-terminated and live for the call.
    let result = unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), flags) };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}
