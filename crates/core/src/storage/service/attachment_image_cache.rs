//! Immutable model-visible PNG bytes, independent of future image encoder versions.
//! Callers must authorize and verify the original attachment before consulting this cache.
use super::*;
use sha2::{Digest, Sha256};
use std::io::Write;

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

impl StorageService {
    fn model_image_cache_root(&self) -> Result<PathBuf, String> {
        let parent = self.attachment_root.parent().ok_or("附件缓存目录无效")?;
        let directory = parent.join("attachment-model-images");
        let root = directory.join("v1");
        for path in [&directory, &root] {
            if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
                return Err("context_image_integrity_mismatch: invalid cache directory".into());
            }
        }
        Ok(root)
    }

    fn model_image_cache_path(
        &self,
        source_digest: &str,
        image_digest: &str,
    ) -> Result<PathBuf, String> {
        if !is_digest(source_digest) || !is_digest(image_digest) {
            return Err("context_image_integrity_mismatch: invalid cache identity".into());
        }
        let root = self.model_image_cache_root()?;
        for path in [&root, &root.join(source_digest)] {
            if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
                return Err("context_image_integrity_mismatch: invalid cache directory".into());
            }
        }
        Ok(root.join(source_digest).join(format!("{image_digest}.png")))
    }

    pub(super) fn cache_model_image(&self, source_path: &Path, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() as u64 > crate::file_input::MAX_AGENT_VISUAL_INPUT_BYTES {
            return Err("模型图片超过缓存预算".into());
        }
        let (_, source_digest) = super::attachment_imports::file_digest(source_path)?;
        let image_digest = format!("{:x}", Sha256::digest(bytes));
        let path = self.model_image_cache_path(&source_digest, &image_digest)?;
        let directory = path.parent().ok_or("附件缓存目录无效")?;
        fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        self.record_model_image_origin(directory, source_path)?;
        if path.exists() {
            self.read_cached_model_image(
                &source_digest,
                &crate::ConversationContextImageRef {
                    attachment_id: String::new(),
                    mime_type: "image/png".into(),
                    sha256: format!("sha256:{image_digest}"),
                },
            )?;
            return Ok(());
        }
        let temporary = directory.join(format!("{}.tmp", Uuid::new_v4()));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| error.to_string())?;
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| error.to_string())?;
            fs::rename(&temporary, &path).map_err(|error| error.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn record_model_image_origin(
        &self,
        directory: &Path,
        source_path: &Path,
    ) -> Result<(), String> {
        // Keep a cheap liveness hint. Forks can share the same content-addressed bytes without
        // copying this index; collection verifies remaining attachment hashes before removal.
        let parent = self
            .attachment_root
            .parent()
            .ok_or("附件缓存目录无效")?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let source_path = source_path
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let relative = source_path
            .strip_prefix(&parent)
            .map_err(|_| "附件缓存来源越界")?;
        if relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err("附件缓存来源无效".into());
        }
        let origin = slash_path(relative);
        let origin_path = directory.join(format!(
            "origin-{:x}.json",
            Sha256::digest(origin.as_bytes())
        ));
        if !origin_path.exists() {
            let temporary = directory.join(format!("{}.tmp", Uuid::new_v4()));
            fs::write(
                &temporary,
                serde_json::to_vec(&origin).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            fs::rename(&temporary, origin_path).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub(super) fn read_cached_model_image(
        &self,
        source_digest: &str,
        reference: &crate::ConversationContextImageRef,
    ) -> Result<Option<Vec<u8>>, String> {
        if reference.mime_type != "image/png" {
            return Ok(None);
        }
        let digest = reference
            .sha256
            .strip_prefix("sha256:")
            .ok_or("图片摘要格式无效")?;
        let path = self.model_image_cache_path(source_digest, digest)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        if !metadata.is_file() || metadata.len() > crate::file_input::MAX_AGENT_VISUAL_INPUT_BYTES {
            return Err("context_image_integrity_mismatch: invalid cached image".into());
        }
        let bytes = fs::read(path).map_err(|error| error.to_string())?;
        if format!("{:x}", Sha256::digest(&bytes)) != digest {
            return Err("context_image_integrity_mismatch: cached image changed".into());
        }
        Ok(Some(bytes))
    }

    /// Run after ordinary attachment and import cleanup, before accepting new imports.
    pub(super) fn cleanup_model_image_cache(&self) -> Result<(), String> {
        let root = self.model_image_cache_root()?;
        if !root.exists() {
            return Ok(());
        }
        let parent = self.attachment_root.parent().ok_or("附件缓存目录无效")?;
        let mut candidates = HashMap::new();
        for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let digest = entry.file_name().to_string_lossy().to_string();
            if !is_digest(&digest)
                || !entry
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_dir()
            {
                continue;
            }
            let mut live = false;
            for origin in fs::read_dir(entry.path()).map_err(|error| error.to_string())? {
                let origin = origin.map_err(|error| error.to_string())?;
                if !origin.file_name().to_string_lossy().starts_with("origin-") {
                    continue;
                }
                let metadata =
                    fs::symlink_metadata(origin.path()).map_err(|error| error.to_string())?;
                if !metadata.is_file() || metadata.len() > 16 * 1024 {
                    return Err("附件缓存来源索引无效".into());
                }
                let relative: String = serde_json::from_slice(
                    &fs::read(origin.path()).map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
                let relative = Path::new(&relative);
                if relative
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
                {
                    return Err("附件缓存来源索引越界".into());
                }
                if fs::symlink_metadata(parent.join(relative))
                    .is_ok_and(|metadata| metadata.is_file())
                {
                    live = true;
                    break;
                }
            }
            if !live {
                candidates.insert(digest, entry.path());
            }
        }
        if candidates.is_empty() {
            return Ok(());
        }
        // Only rare orphan candidates need a streaming hash pass. This also preserves caches
        // inherited by forks whose source conversation was subsequently deleted.
        let paths = {
            let connection = self.state.connection()?;
            let mut statement = connection
                .prepare("SELECT storage_rel_path FROM attachments WHERE kind = 'image'")
                .map_err(storage_error)?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(storage_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(storage_error)?
        };
        for relative in paths {
            if let Some(path) =
                safe_existing_attachment_storage_path(&self.attachment_root, &relative)
            {
                let (_, digest) = super::attachment_imports::file_digest(&path)?;
                if let Some(directory) = candidates.remove(&digest) {
                    self.record_model_image_origin(&directory, &path)?;
                }
                if candidates.is_empty() {
                    break;
                }
            }
        }
        for directory in candidates.values() {
            fs::remove_dir_all(directory).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}
