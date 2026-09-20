//! Durable, opaque attachment imports. Large input bytes never enter a JSON chat request.
use super::*;
use crate::{AttachmentImportInput, AttachmentInputPreview};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{BufReader, Read, Write};

const MAX_CHUNK_BYTES: usize = 512 * 1024;
static IMPORT_LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportManifest {
    version: u32,
    input: AttachmentImportInput,
    complete: bool,
    sha256: Option<String>,
    /// Existing durable attachment sources stay inside the independently validated library.
    storage_rel_path: Option<String>,
}

pub(super) enum AttachmentData {
    Managed {
        path: PathBuf,
        size: u64,
        sha256: String,
    },
}

impl AttachmentData {
    pub(super) fn len(&self) -> u64 {
        match self {
            Self::Managed { size, .. } => *size,
        }
    }

    pub(super) fn write_to(&self, target: &mut fs::File) -> std::io::Result<()> {
        match self {
            Self::Managed { path, size, sha256 } => {
                let mut source = BufReader::new(fs::File::open(path)?);
                let mut buffer = [0u8; 64 * 1024];
                let mut digest = Sha256::new();
                let mut count = 0u64;
                loop {
                    let read = source.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    count = count
                        .checked_add(read as u64)
                        .ok_or_else(|| std::io::Error::other("attachment size overflow"))?;
                    if count > *size {
                        return Err(std::io::Error::other("attachment size changed"));
                    }
                    digest.update(&buffer[..read]);
                    target.write_all(&buffer[..read])?;
                }
                if count != *size || format!("{:x}", digest.finalize()) != *sha256 {
                    return Err(std::io::Error::other("attachment integrity changed"));
                }
                Ok(())
            }
        }
    }

    pub(super) fn matches_file(&self, path: &Path) -> Result<bool, String> {
        if !fs::symlink_metadata(path)
            .map_err(|error| error.to_string())?
            .is_file()
        {
            return Ok(false);
        }
        let (size, hash) = file_digest(path)?;
        Ok(match self {
            Self::Managed {
                size: expected,
                sha256,
                ..
            } => size == *expected && hash == *sha256,
        })
    }
}

pub(super) fn file_digest(path: &Path) -> Result<(u64, String), String> {
    let mut input =
        BufReader::new(fs::File::open(path).map_err(|error| format!("读取附件失败：{error}"))?);
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| format!("读取附件失败：{error}"))?;
        if read == 0 {
            break;
        }
        size = size.checked_add(read as u64).ok_or("附件大小溢出")?;
        digest.update(&buffer[..read]);
    }
    Ok((size, format!("{:x}", digest.finalize())))
}

impl StorageService {
    /// Imports survive crashes and draft recovery. Only old, unreferenced staging entries are
    /// collected, before the Host admits new requests. Unknown/malformed persisted JSON aborts
    /// collection rather than risking removal of a live reference.
    pub(super) fn cleanup_attachment_imports(&self, current_time_ms: i64) -> Result<(), String> {
        const GRACE_MS: i64 = 7 * 24 * 60 * 60 * 1000;
        let root = self.import_root();
        if !root.exists() {
            return Ok(());
        }
        let mut referenced = HashSet::new();
        {
            let connection = self.state.connection()?;
            for sql in [
                "SELECT attachments_json FROM composer_drafts UNION ALL SELECT queued_messages_json FROM composer_drafts",
                "SELECT agent_input_json FROM agent_pending_actions",
                "SELECT checkpoint_json FROM human_interaction_suspensions",
            ] {
                let mut statement = connection.prepare(sql).map_err(storage_error)?;
                let values = statement.query_map([], |row| row.get::<_, String>(0)).map_err(storage_error)?;
                for value in values {
                    collect_managed_refs(&serde_json::from_str(&value.map_err(storage_error)?).map_err(|error| error.to_string())?, &mut referenced);
                }
            }
        }
        for entry in fs::read_dir(&root).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let id = entry.file_name().to_string_lossy().to_string();
            if referenced.contains(&id) || Uuid::parse_str(&id).is_err() {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            if !metadata.is_dir() {
                continue;
            }
            let modified = metadata
                .modified()
                .map_err(|error| error.to_string())?
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_millis();
            if modified < current_time_ms.saturating_sub(GRACE_MS).max(0) as u128 {
                fs::remove_dir_all(entry.path()).map_err(|error| error.to_string())?;
            }
        }
        let indexes = root.join("stored-references");
        if indexes.is_dir() {
            for entry in fs::read_dir(indexes).map_err(|error| error.to_string())? {
                let entry = entry.map_err(|error| error.to_string())?;
                let metadata =
                    fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
                if metadata.is_file() && metadata.len() <= 64 {
                    let id = fs::read_to_string(entry.path()).map_err(|error| error.to_string())?;
                    if Uuid::parse_str(&id).is_ok() && !root.join(&id).exists() {
                        fs::remove_file(entry.path()).map_err(|error| error.to_string())?;
                    }
                }
            }
        }
        Ok(())
    }

    fn import_root(&self) -> PathBuf {
        self.attachment_root
            .parent()
            .unwrap_or(Path::new("."))
            .join("attachment-imports")
            .join("v1")
    }

    fn import_directory(&self, import_id: &str) -> Result<PathBuf, String> {
        let id = Uuid::parse_str(import_id).map_err(|_| "附件引用无效")?;
        if id.to_string() != import_id {
            return Err("附件引用无效".into());
        }
        let path = self.import_root().join(import_id);
        if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err("附件导入目录不能是符号链接".into());
        }
        Ok(path)
    }

    fn read_import_manifest(&self, import_id: &str) -> Result<(PathBuf, ImportManifest), String> {
        let directory = self.import_directory(import_id)?;
        let path = directory.join("manifest.json");
        let metadata = fs::symlink_metadata(&path).map_err(|_| "附件导入记录不存在")?;
        if !metadata.is_file() || metadata.len() > 16 * 1024 {
            return Err("附件导入记录无效".into());
        }
        let manifest: ImportManifest =
            serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
                .map_err(|_| "附件导入记录无效")?;
        if manifest.version != 1 {
            return Err("附件导入版本不支持".into());
        }
        Ok((directory, manifest))
    }

    fn write_import_manifest(directory: &Path, manifest: &ImportManifest) -> Result<(), String> {
        let temporary = directory.join(format!("manifest-{}.tmp", Uuid::new_v4()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        serde_json::to_writer(&mut file, manifest).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&temporary, directory.join("manifest.json")).map_err(|error| error.to_string())
    }

    pub fn begin_attachment_import(&self, input: AttachmentImportInput) -> Result<String, String> {
        if input.id.is_empty()
            || input.id.len() > 256
            || safe_path_component(&input.id, "attachment") != input.id
            || input.name.is_empty()
            || input.name.len() > 255
            || safe_file_name(&input.name, "attachment") != input.name
            || input.size_bytes > i64::MAX as u64
            || input
                .mime_type
                .as_ref()
                .is_some_and(|mime| mime.len() > 255 || mime.chars().any(char::is_control))
        {
            return Err("附件导入元数据无效".into());
        }
        let import_id = Uuid::new_v4().to_string();
        let directory = self.import_directory(&import_id)?;
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("payload"))
            .map_err(|error| error.to_string())?;
        Self::write_import_manifest(
            &directory,
            &ImportManifest {
                version: 1,
                input,
                complete: false,
                sha256: None,
                storage_rel_path: None,
            },
        )?;
        Ok(import_id)
    }

    pub fn append_attachment_import(
        &self,
        import_id: &str,
        offset: u64,
        data: &str,
    ) -> Result<u64, String> {
        if data.len() > MAX_CHUNK_BYTES.div_ceil(3) * 4 {
            return Err("附件导入分块过大".into());
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| "附件分块base64无效")?;
        if bytes.len() > MAX_CHUNK_BYTES {
            return Err("附件导入分块过大".into());
        }
        let _guard = IMPORT_LOCK.lock().map_err(|_| "附件导入锁无效")?;
        let (directory, manifest) = self.read_import_manifest(import_id)?;
        if manifest.complete {
            return Err("附件导入已完成".into());
        }
        let path = directory.join("payload");
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or("附件导入大小溢出")?;
        if !metadata.is_file() || metadata.len() != offset || end > manifest.input.size_bytes {
            return Err("附件导入偏移或长度不匹配".into());
        }
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(|error| error.to_string())?;
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        Ok(end)
    }

    pub fn finish_attachment_import(
        &self,
        import_id: &str,
    ) -> Result<AgentInputAttachment, String> {
        let _guard = IMPORT_LOCK.lock().map_err(|_| "附件导入锁无效")?;
        let (directory, mut manifest) = self.read_import_manifest(import_id)?;
        if !manifest.complete {
            let payload = directory.join("payload");
            let metadata = fs::symlink_metadata(&payload).map_err(|error| error.to_string())?;
            if !metadata.is_file() {
                return Err("附件导入文件无效".into());
            }
            let (size, hash) = file_digest(&payload)?;
            if size != manifest.input.size_bytes {
                return Err("附件导入尚未完成".into());
            }
            fs::File::open(&payload)
                .and_then(|file| file.sync_all())
                .map_err(|error| error.to_string())?;
            if manifest.input.kind == AgentInputAttachmentKind::Image {
                let prepared = crate::file_input::image_delivery::prepare_model_image(
                    BufReader::new(fs::File::open(&payload).map_err(|error| error.to_string())?),
                )
                .map_err(|error| error.to_string())?;
                if manifest.input.mime_type.as_deref() != Some(prepared.source_mime_type) {
                    return Err("附件图片格式与MIME不匹配".into());
                }
                self.cache_model_image(&payload, &prepared.bytes)?;
            }
            manifest.sha256 = Some(hash);
            manifest.complete = true;
            Self::write_import_manifest(&directory, &manifest)?;
        }
        let sha256 = manifest.sha256.as_deref().ok_or("附件导入缺少校验值")?;
        Ok(managed_reference(import_id, manifest.input, sha256))
    }

    pub fn cancel_attachment_import(&self, import_id: &str) -> Result<(), String> {
        let _guard = IMPORT_LOCK.lock().map_err(|_| "附件导入锁无效")?;
        let directory = self.import_directory(import_id)?;
        if !directory.exists() {
            return Ok(());
        }
        let (_, manifest) = self.read_import_manifest(import_id)?;
        // A completed reference may already be durable in a composer draft or pending admission.
        if !manifest.complete {
            fs::remove_dir_all(directory).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub(super) fn attachment_data(
        &self,
        attachment: &AgentInputAttachment,
    ) -> Result<AttachmentData, String> {
        if attachment.encoding != AgentInputAttachmentEncoding::Managed {
            return Err("原始输入附件必须使用托管引用".into());
        }
        let (directory, manifest) = self.read_import_manifest(&attachment.data)?;
        if !manifest.complete
            || manifest.input.kind != attachment.kind
            || manifest.input.name != attachment.name
            || manifest.input.mime_type != attachment.mime_type
            || manifest.input.size_bytes != attachment.size_bytes
            || attachment.truncated == Some(true)
        {
            return Err("附件引用元数据不匹配".into());
        }
        let path = match manifest.storage_rel_path {
            Some(relative) => {
                safe_existing_attachment_storage_path(&self.attachment_root, &relative)
                    .ok_or("附件文件不存在")?
            }
            None => directory.join("payload"),
        };
        let metadata = fs::symlink_metadata(&path).map_err(|_| "附件文件不存在")?;
        if !metadata.is_file() || metadata.len() != attachment.size_bytes {
            return Err("附件引用内容已改变".into());
        }
        Ok(AttachmentData::Managed {
            path,
            size: attachment.size_bytes,
            sha256: manifest.sha256.ok_or("附件引用缺少校验值")?,
        })
    }

    pub fn validate_managed_input_attachment(
        &self,
        attachment: &AgentInputAttachment,
    ) -> Result<(), String> {
        let data = self.attachment_data(attachment)?;
        match &data {
            AttachmentData::Managed { path, .. } if data.matches_file(path)? => Ok(()),
            _ => Err("附件引用内容校验失败".into()),
        }
    }

    /// Compare authenticated managed contents. Invalid references never share an empty identity.
    pub fn input_attachment_content_digest(
        &self,
        attachment: &AgentInputAttachment,
    ) -> Result<String, String> {
        let data = self.attachment_data(attachment)?;
        match &data {
            AttachmentData::Managed { path, sha256, .. } if data.matches_file(path)? => {
                Ok(sha256.clone())
            }
            _ => Err("附件引用内容校验失败".into()),
        }
    }

    pub fn open_validated_managed_input_attachment(
        &self,
        attachment: &AgentInputAttachment,
    ) -> Result<fs::File, String> {
        self.validate_managed_input_attachment(attachment)?;
        let AttachmentData::Managed { path, .. } = self.attachment_data(attachment)?;
        fs::File::open(path).map_err(|error| error.to_string())
    }

    pub(super) fn reference_for_stored_attachment(
        &self,
        attachment: &AttachmentRecord,
    ) -> Result<AgentInputAttachment, String> {
        let source = safe_existing_attachment_storage_path(
            &self.attachment_root,
            &attachment.storage_rel_path,
        )
        .ok_or("附件文件不存在")?;
        let (size, sha256) = file_digest(&source)?;
        if size != attachment.size_bytes {
            return Err("附件文件长度不匹配".into());
        }
        let input = AttachmentImportInput {
            id: attachment.id.clone(),
            kind: agent_attachment_kind(&attachment.kind),
            name: attachment.original_name.clone(),
            mime_type: attachment.mime_type.clone(),
            size_bytes: attachment.size_bytes,
        };
        // Runtime queue idempotency compares its immutable admitted input. Rehydrating a durable
        // attachment must therefore reuse its opaque reference, not mint a new identity per read.
        let _guard = IMPORT_LOCK.lock().map_err(|_| "附件导入锁无效")?;
        let index_key = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(&attachment.storage_rel_path, &input, &sha256))
                    .map_err(|error| error.to_string())?
            )
        );
        let index_directory = self.import_root().join("stored-references");
        fs::create_dir_all(&index_directory).map_err(|error| error.to_string())?;
        let index_path = index_directory.join(&index_key);
        if let Ok(metadata) = fs::symlink_metadata(&index_path) {
            if !metadata.is_file() || metadata.len() > 64 {
                return Err("附件引用索引无效".into());
            }
            let import_id = fs::read_to_string(&index_path).map_err(|error| error.to_string())?;
            if let Ok((_, manifest)) = self.read_import_manifest(&import_id) {
                if manifest.complete
                    && manifest.sha256.as_ref() == Some(&sha256)
                    && manifest.storage_rel_path.as_ref() == Some(&attachment.storage_rel_path)
                    && manifest.input == input
                {
                    return Ok(managed_reference(&import_id, input, &sha256));
                }
                return Err("附件引用索引与内容不一致".into());
            }
        }
        let import_id = Uuid::new_v4().to_string();
        let directory = self.import_directory(&import_id)?;
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        Self::write_import_manifest(
            &directory,
            &ImportManifest {
                version: 1,
                input: input.clone(),
                complete: true,
                sha256: Some(sha256.clone()),
                storage_rel_path: Some(attachment.storage_rel_path.clone()),
            },
        )?;
        let temporary_index = index_directory.join(format!("{index_key}-{import_id}.tmp"));
        fs::write(&temporary_index, import_id.as_bytes()).map_err(|error| error.to_string())?;
        fs::rename(&temporary_index, index_path).map_err(|error| error.to_string())?;
        Ok(managed_reference(&import_id, input, &sha256))
    }

    pub fn load_input_attachment_preview(
        &self,
        attachment: &AgentInputAttachment,
    ) -> Result<Option<AttachmentInputPreview>, String> {
        self.load_input_attachment_preview_for_display(attachment, false)
    }

    pub fn load_input_attachment_preview_for_display(
        &self,
        attachment: &AgentInputAttachment,
        display: bool,
    ) -> Result<Option<AttachmentInputPreview>, String> {
        if attachment.kind != AgentInputAttachmentKind::Image {
            return Ok(None);
        }
        let AttachmentData::Managed { path, .. } = self.attachment_data(attachment)?;
        self.validate_managed_input_attachment(attachment)?;
        let file = fs::File::open(path).map_err(|error| error.to_string())?;
        let data = if display {
            let prepared =
                crate::file_input::image_delivery::prepare_model_image(BufReader::new(file))
                    .map_err(|error| error.to_string())?;
            base64::engine::general_purpose::STANDARD.encode(prepared.bytes)
        } else {
            let url = crate::file_input::image_delivery::thumbnail_data_url(BufReader::new(file))
                .map_err(|error| error.to_string())?;
            url.strip_prefix("data:image/png;base64,")
                .ok_or("图片预览格式无效")?
                .to_string()
        };
        Ok(Some(AttachmentInputPreview {
            mime_type: "image/png".into(),
            data,
        }))
    }
}

fn collect_managed_refs(value: &serde_json::Value, referenced: &mut HashSet<String>) {
    match value {
        serde_json::Value::Object(object) => {
            if object.get("encoding").and_then(serde_json::Value::as_str) == Some("managed") {
                if let Some(id) = object.get("data").and_then(serde_json::Value::as_str) {
                    referenced.insert(id.to_string());
                }
            }
            for value in object.values() {
                collect_managed_refs(value, referenced);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_managed_refs(value, referenced);
            }
        }
        _ => {}
    }
}

fn managed_reference(
    import_id: &str,
    input: AttachmentImportInput,
    content_sha256: &str,
) -> AgentInputAttachment {
    AgentInputAttachment {
        id: input.id,
        kind: input.kind,
        name: input.name,
        mime_type: input.mime_type,
        size_bytes: input.size_bytes,
        encoding: AgentInputAttachmentEncoding::Managed,
        data: import_id.to_string(),
        content_sha256: Some(format!("sha256:{content_sha256}")),
        truncated: None,
    }
}
