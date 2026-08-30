use super::*;
use crate::durable_fs::{atomic_rename_noreplace, sync_directory};
use crate::file_input::MAX_AGENT_VISUAL_INPUT_BYTES;
use crate::image_generation::{
    validate_staged_image, ImageArtifactFormat, DEFAULT_IMAGE_ARTIFACT_MAX_BYTES,
};
use crate::storage::managed_artifact_repository::{
    self, ManagedArtifactGrant, ManagedArtifactKind, ManagedArtifactRecord,
};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};

const MANAGED_ARTIFACT_OBJECTS_DIRECTORY: &str = "objects";
pub const MAX_MANAGED_DOCUMENT_ARTIFACT_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct ManagedArtifactAuthority<'a> {
    pub conversation_id: &'a str,
    pub run_id: &'a str,
    pub call_id: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedManagedArtifact {
    pub kind: ManagedArtifactKind,
    pub format: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub absolute_path: PathBuf,
}

/// Verified bytes and immutable presentation metadata for one generic Artifact authorized by an
/// exact conversation or its trusted Agent task tree. Physical storage paths intentionally never
/// cross this service boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedManagedArtifactContent {
    pub kind: ManagedArtifactKind,
    pub format: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: Vec<u8>,
}

impl PublishedManagedArtifact {
    pub fn read_path(&self) -> String {
        let scheme = match self.kind {
            ManagedArtifactKind::Image => "image-artifact",
            ManagedArtifactKind::Document => "artifact",
        };
        format!("{scheme}://sha256/{}", self.sha256)
    }
}

impl StorageService {
    /// Publishes one already-produced regular file into the shared immutable Artifact store.
    ///
    /// The source path is never persisted. Publication first copies into a private same-directory
    /// staging file, validates the complete staged bytes, atomically commits by content hash, and
    /// only then records the authoritative registry row. A failed registry write can leave only
    /// an unreachable content-addressed object; retrying is idempotent.
    pub fn publish_managed_artifact_file(
        &self,
        source: &Path,
        authority: ManagedArtifactAuthority<'_>,
    ) -> Result<PublishedManagedArtifact, String> {
        validate_authority(authority)?;
        let requested = requested_format(source)?;
        let source_metadata = fs::symlink_metadata(source)
            .map_err(|error| format!("managed output is unavailable: {error}"))?;
        if source_metadata.file_type().is_symlink() || !source_metadata.file_type().is_file() {
            return Err("managed output must be a non-symlink regular file".to_string());
        }
        let max_bytes = match requested {
            RequestedFormat::Image(_) => MAX_AGENT_VISUAL_INPUT_BYTES,
            RequestedFormat::Pdf => MAX_MANAGED_DOCUMENT_ARTIFACT_BYTES,
        };
        if source_metadata.len() == 0 || source_metadata.len() > max_bytes {
            return Err(format!(
                "managed output is empty or exceeds the {} byte file limit",
                max_bytes
            ));
        }

        let objects_root = self
            .managed_artifact_root
            .join(MANAGED_ARTIFACT_OBJECTS_DIRECTORY);
        ensure_private_objects_root(&self.managed_artifact_root, &objects_root)?;
        let staging_path = allocate_staging_path(&objects_root)?;
        let mut staging_guard = StagingGuard::new(staging_path.clone());
        copy_regular_file_bounded(source, &staging_path, max_bytes)?;

        let metadata = match requested {
            RequestedFormat::Image(expected) => {
                let validated = validate_staged_image(
                    &staging_path,
                    Some(expected.media_type()),
                    DEFAULT_IMAGE_ARTIFACT_MAX_BYTES,
                    Some(source_metadata.len()),
                )
                .map_err(|error| format!("managed image output is invalid: {error}"))?;
                if validated.format != expected {
                    return Err(
                        "managed image output extension does not match its content".to_string()
                    );
                }
                ManagedArtifactMetadata {
                    kind: ManagedArtifactKind::Image,
                    format: validated.format.extension().to_string(),
                    media_type: validated.format.media_type().to_string(),
                    size_bytes: validated.size_bytes,
                    sha256: validated.sha256,
                    width: Some(validated.width),
                    height: Some(validated.height),
                }
            }
            RequestedFormat::Pdf => validate_pdf(&staging_path, source_metadata.len())?,
        };
        let extension = match metadata.format.as_str() {
            "jpeg" => "jpg",
            value => value,
        };
        let target_name = format!("{}.{}", metadata.sha256, extension);
        let target_path = objects_root.join(&target_name);
        match atomic_rename_noreplace(&staging_path, &target_path) {
            Ok(()) => {
                staging_guard.disarm();
                sync_directory(&objects_root)
                    .map_err(|error| format!("managed Artifact directory sync failed: {error}"))?;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                verify_existing_object(&target_path, &metadata)?;
            }
            Err(error) => {
                return Err(format!(
                    "managed Artifact could not be atomically published: {error}"
                ))
            }
        }
        verify_existing_object(&target_path, &metadata)?;

        let record = ManagedArtifactRecord {
            artifact_id: format!("sha256:{}", metadata.sha256),
            kind: metadata.kind,
            storage_relative_path: format!("{MANAGED_ARTIFACT_OBJECTS_DIRECTORY}/{target_name}"),
            format: metadata.format.clone(),
            media_type: metadata.media_type.clone(),
            size_bytes: metadata.size_bytes,
            sha256: metadata.sha256.clone(),
            width: metadata.width,
            height: metadata.height,
            created_at: now_ms(),
        };
        let mut connection = self.state.connection()?;
        let registered = managed_artifact_repository::register(&mut connection, &record)
            .map_err(storage_error)?;
        managed_artifact_repository::grant(
            &mut connection,
            &ManagedArtifactGrant {
                artifact_id: registered.artifact_id.clone(),
                conversation_id: authority.conversation_id.to_string(),
                run_id: authority.run_id.to_string(),
                call_id: authority.call_id.to_string(),
                created_at: now_ms(),
            },
        )
        .map_err(storage_error)?;
        Ok(PublishedManagedArtifact {
            kind: registered.kind,
            format: registered.format,
            media_type: registered.media_type,
            size_bytes: registered.size_bytes,
            sha256: registered.sha256,
            width: registered.width,
            height: registered.height,
            absolute_path: target_path,
        })
    }

    pub(crate) fn resolve_published_managed_artifact_input(
        &self,
        artifact_id: &str,
        conversation_id: Option<&str>,
    ) -> Result<Option<ResolvedGeneratedArtifactInput>, String> {
        let Some(conversation_id) = conversation_id else {
            return Ok(None);
        };
        let connection = self.state.connection()?;
        let Some(record) =
            managed_artifact_repository::find_authorized(&connection, artifact_id, conversation_id)
                .map_err(storage_error)?
        else {
            return Ok(None);
        };
        let relative = safe_artifact_relative_path(&record.storage_relative_path)?;
        Ok(Some(ResolvedGeneratedArtifactInput {
            path: self.managed_artifact_root.join(relative),
            size_bytes: record.size_bytes,
            sha256: record.sha256,
            kind: ResolvedGeneratedArtifactKind::Managed(record.kind),
        }))
    }

    /// Reads a generic Artifact only when the current conversation owns an explicit publication
    /// grant or belongs to that grant owner's Agent task tree. Independent image-generation
    /// Artifacts use their existing store and are not handled here.
    pub fn read_authorized_managed_artifact(
        &self,
        artifact_id: &str,
        conversation_id: &str,
    ) -> Result<Option<AuthorizedManagedArtifactContent>, String> {
        let connection = self.state.connection()?;
        let Some(record) =
            managed_artifact_repository::find_authorized(&connection, artifact_id, conversation_id)
                .map_err(storage_error)?
        else {
            return Ok(None);
        };
        let relative = safe_artifact_relative_path(&record.storage_relative_path)?;
        let path = self.managed_artifact_root.join(relative);
        let mut file = open_regular_file_no_follow(&path)
            .map_err(|error| format!("managed Artifact could not be opened safely: {error}"))?;
        let actual_size = file
            .metadata()
            .map_err(|error| format!("managed Artifact metadata is unavailable: {error}"))?
            .len();
        if actual_size != record.size_bytes {
            return Err("managed Artifact size no longer matches its journal identity".to_string());
        }
        let max_bytes = match record.kind {
            ManagedArtifactKind::Image => MAX_AGENT_VISUAL_INPUT_BYTES,
            ManagedArtifactKind::Document => MAX_MANAGED_DOCUMENT_ARTIFACT_BYTES,
        };
        if actual_size == 0 || actual_size > max_bytes {
            return Err("managed Artifact exceeds its immutable read limit".to_string());
        }
        let capacity = usize::try_from(actual_size)
            .map_err(|_| "managed Artifact is too large for this platform".to_string())?;
        let mut bytes = Vec::with_capacity(capacity);
        file.read_to_end(&mut bytes)
            .map_err(|error| format!("managed Artifact could not be read: {error}"))?;
        if bytes.len() as u64 != actual_size
            || format!("{:x}", Sha256::digest(&bytes)) != record.sha256
        {
            return Err(
                "managed Artifact bytes no longer match their journal identity".to_string(),
            );
        }
        Ok(Some(AuthorizedManagedArtifactContent {
            kind: record.kind,
            format: record.format,
            media_type: record.media_type,
            size_bytes: record.size_bytes,
            sha256: record.sha256,
            width: record.width,
            height: record.height,
            bytes,
        }))
    }
}

fn validate_authority(authority: ManagedArtifactAuthority<'_>) -> Result<(), String> {
    for (label, value) in [
        ("conversation", authority.conversation_id),
        ("run", authority.run_id),
        ("call", authority.call_id),
    ] {
        if value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
            return Err(format!("managed Artifact {label} authority is invalid"));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum RequestedFormat {
    Image(ImageArtifactFormat),
    Pdf,
}

fn requested_format(path: &Path) -> Result<RequestedFormat, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "managed output must have a supported file extension".to_string())?;
    match extension.as_str() {
        "png" => Ok(RequestedFormat::Image(ImageArtifactFormat::Png)),
        "jpg" | "jpeg" => Ok(RequestedFormat::Image(ImageArtifactFormat::Jpeg)),
        "webp" => Ok(RequestedFormat::Image(ImageArtifactFormat::Webp)),
        "pdf" => Ok(RequestedFormat::Pdf),
        _ => Err(format!(
            "managed output type .{extension} is unsupported; expected PNG, JPEG, WebP, or PDF"
        )),
    }
}

fn safe_artifact_relative_path(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if value.trim().is_empty() || path.is_absolute() {
        return Err("managed Artifact registry contains an invalid storage path".to_string());
    }
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("managed Artifact registry contains an unsafe storage path".to_string())
            }
        }
    }
    if relative.as_os_str().is_empty() {
        Err("managed Artifact registry contains an empty storage path".to_string())
    } else {
        Ok(relative)
    }
}

#[derive(Debug, Clone)]
struct ManagedArtifactMetadata {
    kind: ManagedArtifactKind,
    format: String,
    media_type: String,
    size_bytes: u64,
    sha256: String,
    width: Option<u32>,
    height: Option<u32>,
}

fn validate_pdf(path: &Path, expected_size: u64) -> Result<ManagedArtifactMetadata, String> {
    let mut file = open_regular_file_no_follow(path)
        .map_err(|error| format!("managed PDF output is unavailable: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("managed PDF metadata is unavailable: {error}"))?;
    if metadata.len() != expected_size
        || metadata.len() == 0
        || metadata.len() > MAX_MANAGED_DOCUMENT_ARTIFACT_BYTES
    {
        return Err("managed PDF output changed or exceeds the file limit".to_string());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("managed PDF output could not be read: {error}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != metadata.len() {
        return Err("managed PDF output changed while it was being validated".to_string());
    }
    let header_end = bytes.len().min(1024);
    let header = &bytes[..header_end];
    let tail_start = bytes.len().saturating_sub(4096);
    if !header.windows(5).any(|window| window == b"%PDF-")
        || !bytes[tail_start..]
            .windows(5)
            .any(|window| window == b"%%EOF")
    {
        return Err("managed PDF output is missing a valid PDF header or trailer".to_string());
    }
    let sha256 = hex_sha256(&bytes);
    Ok(ManagedArtifactMetadata {
        kind: ManagedArtifactKind::Document,
        format: "pdf".to_string(),
        media_type: "application/pdf".to_string(),
        size_bytes: metadata.len(),
        sha256,
        width: None,
        height: None,
    })
}

fn copy_regular_file_bounded(source: &Path, target: &Path, max_bytes: u64) -> Result<(), String> {
    let mut input = open_regular_file_no_follow(source)
        .map_err(|error| format!("managed output could not be opened safely: {error}"))?;
    let expected_size = input
        .metadata()
        .map_err(|error| format!("managed output metadata is unavailable: {error}"))?
        .len();
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| format!("managed Artifact staging could not be created: {error}"))?;
    let copied = io::copy(
        &mut std::io::Read::by_ref(&mut input).take(max_bytes.saturating_add(1)),
        &mut output,
    )
    .map_err(|error| format!("managed output could not be staged: {error}"))?;
    if copied != expected_size || copied > max_bytes {
        return Err(
            "managed output changed while being staged or exceeded the file limit".to_string(),
        );
    }
    output
        .flush()
        .and_then(|()| output.sync_all())
        .map_err(|error| format!("managed Artifact staging could not be persisted: {error}"))
}

fn verify_existing_object(path: &Path, expected: &ManagedArtifactMetadata) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("managed Artifact object is unavailable: {error}"))?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() != expected.size_bytes
    {
        return Err("managed Artifact object conflicts with its frozen identity".to_string());
    }
    let mut file = open_regular_file_no_follow(path)
        .map_err(|error| format!("managed Artifact object could not be opened: {error}"))?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut HashWriter(&mut hasher))
        .map_err(|error| format!("managed Artifact object could not be verified: {error}"))?;
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected.sha256 {
        return Err("managed Artifact object hash conflicts with its frozen identity".to_string());
    }
    Ok(())
}

struct HashWriter<'a>(&'a mut Sha256);

impl Write for HashWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.update(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn ensure_private_objects_root(root: &Path, objects_root: &Path) -> Result<(), String> {
    fs::create_dir_all(root)
        .map_err(|error| format!("managed Artifact root could not be created: {error}"))?;
    reject_symlink_directory(root)?;
    set_private_directory_permissions(root)?;
    fs::create_dir_all(objects_root)
        .map_err(|error| format!("managed Artifact object root could not be created: {error}"))?;
    reject_symlink_directory(objects_root)?;
    set_private_directory_permissions(objects_root)?;
    Ok(())
}

fn reject_symlink_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("managed Artifact directory is unavailable: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        Err("managed Artifact path is not a safe directory".to_string())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("managed Artifact directory could not be made private: {error}"))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn allocate_staging_path(objects_root: &Path) -> Result<PathBuf, String> {
    for _ in 0..32 {
        let path = objects_root.join(format!(".managed-staging-{}", Uuid::new_v4()));
        if fs::symlink_metadata(&path).is_err() {
            return Ok(path);
        }
    }
    Err("managed Artifact staging name could not be allocated".to_string())
}

fn open_regular_file_no_follow(path: &Path) -> io::Result<File> {
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
    let file = options.open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "path is not a regular file",
        ));
    }
    Ok(file)
}

struct StagingGuard {
    path: PathBuf,
    armed: bool,
}

impl StagingGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat};
    use std::io::Cursor;

    fn test_storage(root: &Path) -> StorageService {
        StorageService::open(&root.join("storage.sqlite")).unwrap()
    }

    fn png_bytes() -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::new_rgba8(2, 3)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn publishes_images_and_pdfs_into_the_generic_content_addressed_store() {
        let root = tempfile::tempdir().unwrap();
        let storage = test_storage(root.path());
        let image = root.path().join("page.png");
        fs::write(&image, png_bytes()).unwrap();
        let authority = ManagedArtifactAuthority {
            conversation_id: "conversation-1",
            run_id: "run-1",
            call_id: "call-1",
        };
        storage
            .state
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO conversations (id, title, created_at, updated_at)
             VALUES ('conversation-1', 'test', 1, 1)",
                [],
            )
            .unwrap();
        let image_artifact = storage
            .publish_managed_artifact_file(&image, authority)
            .unwrap();
        assert_eq!(image_artifact.kind, ManagedArtifactKind::Image);
        assert!(image_artifact
            .read_path()
            .starts_with("image-artifact://sha256/"));

        let pdf = root.path().join("report.pdf");
        fs::write(&pdf, b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\n%%EOF\n").unwrap();
        let pdf_artifact = storage
            .publish_managed_artifact_file(&pdf, authority)
            .unwrap();
        assert_eq!(pdf_artifact.kind, ManagedArtifactKind::Document);
        assert!(pdf_artifact.read_path().starts_with("artifact://sha256/"));
        assert!(image_artifact.absolute_path.is_file());
        assert!(pdf_artifact.absolute_path.is_file());
        assert!(image_artifact
            .absolute_path
            .starts_with(root.path().join("image-generation-artifacts/objects")));
        assert!(pdf_artifact
            .absolute_path
            .starts_with(root.path().join("image-generation-artifacts/objects")));
        let image_id = format!("sha256:{}", image_artifact.sha256);
        assert!(storage
            .resolve_published_generated_artifact_input(&image_id, Some("conversation-1"))
            .unwrap()
            .is_some());
        assert!(storage
            .resolve_published_generated_artifact_input(&image_id, Some("conversation-2"))
            .unwrap()
            .is_none());
        assert!(storage
            .resolve_published_generated_artifact_input(&image_id, None)
            .unwrap()
            .is_none());
    }

    #[test]
    fn deduplicates_identical_content_and_rejects_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let storage = test_storage(root.path());
        storage
            .state
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO conversations (id, title, created_at, updated_at)
             VALUES ('conversation-1', 'test', 1, 1)",
                [],
            )
            .unwrap();
        let authority = ManagedArtifactAuthority {
            conversation_id: "conversation-1",
            run_id: "run-1",
            call_id: "call-1",
        };
        let first = root.path().join("first.pdf");
        let second = root.path().join("second.pdf");
        let bytes = b"%PDF-1.7\n%%EOF\n";
        fs::write(&first, bytes).unwrap();
        fs::write(&second, bytes).unwrap();
        let first = storage
            .publish_managed_artifact_file(&first, authority)
            .unwrap();
        let second = storage
            .publish_managed_artifact_file(&second, authority)
            .unwrap();
        assert_eq!(first.read_path(), second.read_path());
        assert_eq!(first.absolute_path, second.absolute_path);

        #[cfg(unix)]
        {
            let link = root.path().join("link.pdf");
            std::os::unix::fs::symlink(&first.absolute_path, &link).unwrap();
            assert!(storage
                .publish_managed_artifact_file(&link, authority)
                .is_err());
        }
    }

    #[test]
    fn rejects_images_larger_than_the_read_image_delivery_limit() {
        let root = tempfile::tempdir().unwrap();
        let storage = test_storage(root.path());
        let image = root.path().join("oversized.png");
        let file = File::create(&image).unwrap();
        file.set_len(MAX_AGENT_VISUAL_INPUT_BYTES + 1).unwrap();
        let error = storage
            .publish_managed_artifact_file(
                &image,
                ManagedArtifactAuthority {
                    conversation_id: "conversation-1",
                    run_id: "run-1",
                    call_id: "call-1",
                },
            )
            .unwrap_err();
        assert!(error.contains(&MAX_AGENT_VISUAL_INPUT_BYTES.to_string()));
    }
}
