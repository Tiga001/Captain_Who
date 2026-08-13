use super::*;

pub(super) fn input_attachment_bytes(attachment: &AgentInputAttachment) -> Result<Vec<u8>, String> {
    match attachment.encoding {
        AgentInputAttachmentEncoding::Utf8 => Ok(attachment.data.as_bytes().to_vec()),
        AgentInputAttachmentEncoding::Base64 => base64::engine::general_purpose::STANDARD
            .decode(attachment.data.as_bytes())
            .map_err(|error| format!("附件 base64 数据无效：{error}")),
    }
}

pub(super) fn input_attachment_kind_label(kind: AgentInputAttachmentKind) -> &'static str {
    match kind {
        AgentInputAttachmentKind::File => "file",
        AgentInputAttachmentKind::Image => "image",
    }
}

pub(super) fn agent_attachment_kind(kind: &str) -> AgentInputAttachmentKind {
    match kind {
        "image" => AgentInputAttachmentKind::Image,
        _ => AgentInputAttachmentKind::File,
    }
}

pub(super) fn agent_attachment_reference(record: AttachmentRecord) -> AgentAttachmentReference {
    let read_path = attachment_read_path(&record.id, &record.original_name);
    AgentAttachmentReference {
        id: record.id,
        conversation_id: record.conversation_id,
        message_id: record.message_id,
        project_id: record.project_id,
        kind: agent_attachment_kind(&record.kind),
        name: record.original_name,
        mime_type: record.mime_type,
        size_bytes: record.size_bytes,
        read_path,
        storage_rel_path: record.storage_rel_path,
        created_at: record.created_at,
    }
}

pub(super) fn attachment_storage_rel_path(
    conversation_id: &str,
    message_id: &str,
    attachment_id: &str,
    original_name: &str,
) -> PathBuf {
    PathBuf::from("conversations")
        .join(safe_path_component(conversation_id, "conversation"))
        .join(safe_path_component(message_id, "message"))
        .join(safe_path_component(attachment_id, "attachment"))
        .join(safe_file_name(original_name, attachment_id))
}

pub(super) fn attachment_read_path(attachment_id: &str, original_name: &str) -> String {
    format!(
        "@attachments/{}/{}",
        safe_path_component(attachment_id, "attachment"),
        safe_file_name(original_name, attachment_id)
    )
}

pub(super) fn image_preview_mime_type(attachment: &AttachmentRecord) -> Option<String> {
    if attachment.kind != "image" {
        return None;
    }

    attachment
        .mime_type
        .as_deref()
        .filter(|mime_type| mime_type.starts_with("image/"))
        .map(ToString::to_string)
}

pub(super) fn safe_existing_attachment_storage_path(
    attachment_root: &Path,
    storage_rel_path: &str,
) -> Option<PathBuf> {
    let storage_path = safe_attachment_storage_path(attachment_root, storage_rel_path)?;
    let root = attachment_root.canonicalize().ok()?;
    let canonical = storage_path.canonicalize().ok()?;
    if canonical.starts_with(root) {
        Some(canonical)
    } else {
        None
    }
}

pub(super) fn safe_attachment_storage_path(
    attachment_root: &Path,
    storage_rel_path: &str,
) -> Option<PathBuf> {
    let relative_path = Path::new(storage_rel_path);
    if relative_path.is_absolute() {
        return None;
    }
    if relative_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) {
        return None;
    }

    Some(attachment_root.join(relative_path))
}

pub(super) fn safe_file_name(name: &str, fallback: &str) -> String {
    let file_name = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback);

    safe_path_component(file_name, fallback)
}

pub(super) fn safe_path_component(value: &str, fallback: &str) -> String {
    let sanitized = value
        .trim()
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '\0' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>();

    let sanitized = sanitized.trim();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        fallback.to_string()
    } else {
        sanitized.to_string()
    }
}

pub(super) fn slash_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn ensure_project_reference_exists(
    connection: &rusqlite::Connection,
    project_id: Option<&str>,
) -> Result<(), String> {
    let Some(project_id) = project_id else {
        return Ok(());
    };
    if project_repository::project_exists(connection, project_id).map_err(storage_error)? {
        Ok(())
    } else {
        Err(format!("项目已不存在，拒绝保存关联数据：{project_id}"))
    }
}

pub(super) fn ensure_conversation_exists(
    connection: &rusqlite::Connection,
    conversation_id: &str,
) -> Result<(), String> {
    if chat_repository::conversation_exists(connection, conversation_id).map_err(storage_error)? {
        Ok(())
    } else {
        Err(format!("对话已不存在，拒绝保存关联数据：{conversation_id}"))
    }
}

pub(super) fn orphan_scan_relative_path(attachment_root: &Path, path: &Path) -> Option<String> {
    let relative_path = path.strip_prefix(attachment_root).ok()?;
    let parts = relative_path
        .components()
        .map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some(parts.join("/"))
}

impl StorageService {
    /// Resolves only the durable Conversation owner for an attachment. Host authorization uses
    /// this narrow lookup before any legacy attachment-content read crosses the RPC boundary.
    pub fn attachment_conversation_id(
        &self,
        attachment_id: &str,
    ) -> Result<Option<String>, String> {
        let attachment_id = attachment_id.trim();
        if attachment_id.is_empty() {
            return Ok(None);
        }
        let connection = self.state.connection()?;
        attachment_repository::get_attachment(&connection, attachment_id)
            .map(|record| record.map(|record| record.conversation_id))
            .map_err(storage_error)
    }

    pub fn load_attachment_image(
        &self,
        attachment_id: &str,
    ) -> Result<Option<AttachmentImageRecord>, String> {
        let attachment_id = attachment_id.trim();
        if attachment_id.is_empty() {
            return Ok(None);
        }

        let connection = self.state.connection()?;
        let Some(attachment) = attachment_repository::get_attachment(&connection, attachment_id)
            .map_err(storage_error)?
        else {
            return Ok(None);
        };

        let Some(mime_type) = image_preview_mime_type(&attachment) else {
            return Ok(None);
        };
        let Some(storage_path) = safe_existing_attachment_storage_path(
            &self.attachment_root,
            &attachment.storage_rel_path,
        ) else {
            return Ok(None);
        };

        let bytes = match fs::read(storage_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("读取图片附件失败：{error}")),
        };

        Ok(Some(AttachmentImageRecord {
            id: attachment.id,
            name: attachment.original_name,
            mime_type,
            size_bytes: bytes.len() as u64,
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
            created_at: attachment.created_at,
        }))
    }

    pub fn save_input_attachments(
        &self,
        conversation_id: &str,
        message_id: &str,
        project_id: Option<&str>,
        attachments: &[AgentInputAttachment],
        created_at: i64,
    ) -> Result<(), String> {
        if attachments.is_empty() {
            return Ok(());
        }

        fs::create_dir_all(&self.attachment_root)
            .map_err(|error| format!("创建附件库目录失败：{error}"))?;
        let connection = self.state.connection()?;
        ensure_conversation_exists(&connection, conversation_id)?;
        ensure_project_reference_exists(&connection, project_id)?;

        for attachment in attachments {
            let bytes = input_attachment_bytes(attachment)?;
            let attachment_id = safe_path_component(&attachment.id, "attachment");
            let storage_rel_path = attachment_storage_rel_path(
                conversation_id,
                message_id,
                &attachment_id,
                &attachment.name,
            );
            let storage_path = self.attachment_root.join(&storage_rel_path);
            let parent = storage_path
                .parent()
                .ok_or_else(|| "附件存储路径无效。".to_string())?;
            fs::create_dir_all(parent).map_err(|error| format!("创建附件目录失败：{error}"))?;
            fs::write(&storage_path, &bytes).map_err(|error| format!("写入附件失败：{error}"))?;

            let record = AttachmentRecord {
                id: attachment_id,
                conversation_id: conversation_id.to_string(),
                message_id: message_id.to_string(),
                project_id: project_id.map(ToString::to_string),
                kind: input_attachment_kind_label(attachment.kind).to_string(),
                original_name: attachment.name.clone(),
                mime_type: attachment.mime_type.clone(),
                size_bytes: bytes.len() as u64,
                storage_rel_path: slash_path(&storage_rel_path),
                created_at,
            };
            attachment_repository::save_attachment(&connection, &record).map_err(storage_error)?;
        }

        Ok(())
    }

    /// Persists every guidance attachment and its ownership journal before queue admission.
    ///
    /// Files are written first, then attachment rows, guidance identity, and ownership rows commit
    /// in one SQLite transaction. Any pre-commit failure removes the newly written files. A crash
    /// between file publication and the database commit leaves only unreferenced files, which the
    /// existing startup orphan scan removes.
    pub fn store_agent_run_guidance_with_attachments(
        &self,
        record: AgentRunGuidanceRecord,
        project_id: Option<&str>,
        attachments: &[AgentInputAttachment],
    ) -> Result<AgentRunGuidanceStoreOutcome, String> {
        if attachments.is_empty() {
            return self.store_agent_run_guidance(record);
        }
        let attachment_ids = attachments
            .iter()
            .map(|attachment| attachment.id.clone())
            .collect::<Vec<_>>();
        if record.attachment_ids != attachment_ids {
            return Err(
                "guidance attachment ownership does not match the persisted attachment order"
                    .to_string(),
            );
        }

        fs::create_dir_all(&self.attachment_root)
            .map_err(|error| format!("创建附件库目录失败：{error}"))?;
        let mut connection = self.state.connection()?;
        ensure_conversation_exists(&connection, &record.conversation_id)?;
        ensure_project_reference_exists(&connection, project_id)?;

        let mut prepared = Vec::with_capacity(attachments.len());
        for attachment in attachments {
            if attachment_repository::get_attachment(&connection, &attachment.id)
                .map_err(storage_error)?
                .is_some()
            {
                return Err(format!("附件 id 已存在：{}", attachment.id));
            }
            let bytes = input_attachment_bytes(attachment)?;
            let storage_rel_path = attachment_storage_rel_path(
                &record.conversation_id,
                &record.assistant_message_id,
                &attachment.id,
                &attachment.name,
            );
            let storage_path = self.attachment_root.join(&storage_rel_path);
            prepared.push((
                AttachmentRecord {
                    id: attachment.id.clone(),
                    conversation_id: record.conversation_id.clone(),
                    // The assistant message is only the lifecycle anchor. The explicit
                    // agent_run_guidance_attachments relation is the authoritative owner.
                    message_id: record.assistant_message_id.clone(),
                    project_id: project_id.map(ToString::to_string),
                    kind: input_attachment_kind_label(attachment.kind).to_string(),
                    original_name: attachment.name.clone(),
                    mime_type: attachment.mime_type.clone(),
                    size_bytes: bytes.len() as u64,
                    storage_rel_path: slash_path(&storage_rel_path),
                    created_at: record.created_at,
                },
                storage_path,
                bytes,
            ));
        }

        let mut written_paths = Vec::with_capacity(prepared.len());
        for (_, storage_path, bytes) in &prepared {
            let parent = storage_path
                .parent()
                .ok_or_else(|| "附件存储路径无效。".to_string())?;
            fs::create_dir_all(parent).map_err(|error| format!("创建附件目录失败：{error}"))?;
            let write_result = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(storage_path)
                .and_then(|mut file| {
                    use std::io::Write;
                    file.write_all(bytes)?;
                    file.sync_all()
                });
            if let Err(error) = write_result {
                cleanup_new_guidance_attachment_files(self, &written_paths);
                return Err(format!("写入引导附件失败：{error}"));
            }
            written_paths.push(storage_path.clone());
        }

        let database_result = (|| -> Result<AgentRunGuidanceStoreOutcome, String> {
            let transaction = connection.transaction().map_err(storage_error)?;
            for (attachment, _, _) in &prepared {
                attachment_repository::insert_attachment(&transaction, attachment)
                    .map_err(storage_error)?;
            }
            let outcome = guidance_repository::store_guidance_in_connection(&transaction, &record)
                .map_err(storage_error)?;
            if outcome != AgentRunGuidanceStoreOutcome::Inserted {
                return Err(
                    "guidance identity changed while attachment admission was committing"
                        .to_string(),
                );
            }
            transaction.commit().map_err(storage_error)?;
            Ok(outcome)
        })();

        if database_result.is_err() {
            cleanup_new_guidance_attachment_files(self, &written_paths);
        }
        database_result
    }

    pub fn load_input_attachments(
        &self,
        attachment_ids: &[String],
    ) -> Result<Vec<AgentInputAttachment>, String> {
        let connection = self.state.connection()?;
        let mut attachments = Vec::new();

        for attachment_id in attachment_ids {
            let attachment = attachment_repository::get_attachment(&connection, attachment_id)
                .map_err(storage_error)?
                .ok_or_else(|| format!("附件不存在：{attachment_id}"))?;
            let storage_path = safe_existing_attachment_storage_path(
                &self.attachment_root,
                &attachment.storage_rel_path,
            )
            .ok_or_else(|| format!("附件文件不存在：{}", attachment.original_name))?;
            let bytes = fs::read(&storage_path)
                .map_err(|error| format!("读取附件失败 {}: {error}", storage_path.display()))?;

            attachments.push(AgentInputAttachment {
                id: attachment.id,
                kind: agent_attachment_kind(&attachment.kind),
                name: attachment.original_name,
                mime_type: attachment.mime_type,
                size_bytes: attachment.size_bytes,
                encoding: AgentInputAttachmentEncoding::Base64,
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
                truncated: None,
            });
        }

        Ok(attachments)
    }

    pub fn build_attachment_library_context(
        &self,
        conversation_id: &str,
        project_id: Option<&str>,
    ) -> Result<AgentAttachmentLibraryContext, String> {
        self.build_attachment_library_context_inner(conversation_id, project_id, None)
    }

    pub fn build_attachment_library_context_for_active_run(
        &self,
        conversation_id: &str,
        project_id: Option<&str>,
        run_id: &str,
    ) -> Result<AgentAttachmentLibraryContext, String> {
        self.build_attachment_library_context_inner(conversation_id, project_id, Some(run_id))
    }

    fn build_attachment_library_context_inner(
        &self,
        conversation_id: &str,
        project_id: Option<&str>,
        admitted_run_id: Option<&str>,
    ) -> Result<AgentAttachmentLibraryContext, String> {
        let connection = self.state.connection()?;
        let conversation_attachments =
            attachment_repository::list_conversation_attachments_for_library(
                &connection,
                conversation_id,
                admitted_run_id,
            )
            .map_err(storage_error)?
            .into_iter()
            .map(agent_attachment_reference)
            .collect::<Vec<_>>();
        let project_attachments = if let Some(project_id) = project_id {
            attachment_repository::list_project_attachments_for_library_excluding_conversation(
                &connection,
                project_id,
                conversation_id,
            )
            .map_err(storage_error)?
            .into_iter()
            .map(agent_attachment_reference)
            .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        Ok(AgentAttachmentLibraryContext {
            root_path: Some(self.attachment_root.to_string_lossy().to_string()),
            conversation_id: Some(conversation_id.to_string()),
            project_id: project_id.map(ToString::to_string),
            conversation_attachments,
            project_attachments,
        })
    }

    pub(super) fn attach_message_attachments(
        &self,
        connection: &rusqlite::Connection,
        conversations: &mut [ChatConversationRecord],
    ) -> Result<(), String> {
        for conversation in conversations {
            let attachments = attachment_repository::list_ordinary_conversation_attachments(
                connection,
                &conversation.id,
            )
            .map_err(storage_error)?;
            if attachments.is_empty() {
                continue;
            }

            let mut attachments_by_message_id: HashMap<String, Vec<ChatMessageAttachmentRecord>> =
                HashMap::new();
            for attachment in attachments {
                attachments_by_message_id
                    .entry(attachment.message_id.clone())
                    .or_default()
                    .push(self.chat_message_attachment_record(attachment));
            }

            for message in &mut conversation.messages {
                if let Some(attachments) = attachments_by_message_id.remove(&message.id) {
                    message.attachments = attachments;
                }
            }
        }

        Ok(())
    }

    pub(super) fn chat_message_attachment_record(
        &self,
        attachment: AttachmentRecord,
    ) -> ChatMessageAttachmentRecord {
        let preview_mime_type = image_preview_mime_type(&attachment);
        let preview_data = preview_mime_type
            .as_ref()
            .and_then(|_| self.read_attachment_preview_data(&attachment));

        ChatMessageAttachmentRecord {
            id: attachment.id,
            kind: attachment.kind,
            name: attachment.original_name,
            mime_type: attachment.mime_type,
            size_bytes: attachment.size_bytes,
            preview_data,
            preview_mime_type,
            created_at: attachment.created_at,
        }
    }

    pub(super) fn read_attachment_preview_data(
        &self,
        attachment: &AttachmentRecord,
    ) -> Option<String> {
        let storage_path = safe_existing_attachment_storage_path(
            &self.attachment_root,
            &attachment.storage_rel_path,
        )?;
        let bytes = fs::read(storage_path).ok()?;
        Some(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub(super) fn cleanup_attachment_files(
        &self,
        attachments: Vec<AttachmentRecord>,
    ) -> Result<(), String> {
        let mut errors = Vec::new();

        for attachment in attachments {
            let Some(storage_path) =
                safe_attachment_storage_path(&self.attachment_root, &attachment.storage_rel_path)
            else {
                errors.push(format!("附件路径无效：{}", attachment.storage_rel_path));
                continue;
            };

            match fs::remove_file(&storage_path) {
                Ok(()) => self.prune_empty_attachment_dirs(storage_path.parent(), &mut errors),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.prune_empty_attachment_dirs(storage_path.parent(), &mut errors);
                }
                Err(error) => errors.push(format!("{}: {error}", attachment.storage_rel_path)),
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    pub(super) fn cleanup_orphan_attachment_files(
        &self,
        connection: &rusqlite::Connection,
    ) -> Result<(), String> {
        if !self.attachment_root.exists() {
            return Ok(());
        }

        let referenced_paths = attachment_repository::list_attachment_storage_rel_paths(connection)
            .map_err(storage_error)?
            .into_iter()
            .collect::<HashSet<_>>();
        let mut errors = Vec::new();
        self.cleanup_orphan_attachment_dir(&self.attachment_root, &referenced_paths, &mut errors);

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    pub(super) fn cleanup_orphan_attachment_dir(
        &self,
        current_dir: &Path,
        referenced_paths: &HashSet<String>,
        errors: &mut Vec<String>,
    ) {
        let entries = match fs::read_dir(current_dir) {
            Ok(entries) => entries,
            Err(error) => {
                errors.push(format!(
                    "读取附件目录失败 {}: {error}",
                    current_dir.display()
                ));
                return;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    errors.push(format!(
                        "读取附件目录项失败 {}: {error}",
                        current_dir.display()
                    ));
                    continue;
                }
            };
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    errors.push(format!("读取附件路径失败 {}: {error}", path.display()));
                    continue;
                }
            };
            let file_type = metadata.file_type();

            if file_type.is_dir() {
                self.cleanup_orphan_attachment_dir(&path, referenced_paths, errors);
                self.remove_empty_attachment_dir(&path, errors);
                continue;
            }

            if !file_type.is_file() && !file_type.is_symlink() {
                continue;
            }

            let Some(relative_path) = orphan_scan_relative_path(&self.attachment_root, &path)
            else {
                errors.push(format!("附件路径不在附件目录内：{}", path.display()));
                continue;
            };

            if referenced_paths.contains(&relative_path) {
                continue;
            }

            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => errors.push(format!("删除孤儿附件失败 {relative_path}: {error}")),
            }
        }
    }

    pub(super) fn prune_empty_attachment_dirs(
        &self,
        start: Option<&Path>,
        errors: &mut Vec<String>,
    ) {
        let Some(mut current) = start.map(Path::to_path_buf) else {
            return;
        };

        while current != self.attachment_root {
            match fs::remove_dir(&current) {
                Ok(()) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                    ) =>
                {
                    break;
                }
                Err(error) => {
                    errors.push(format!("{}: {error}", current.display()));
                    break;
                }
            }

            if !current.pop() {
                break;
            }
        }
    }

    pub(super) fn remove_empty_attachment_dir(&self, path: &Path, errors: &mut Vec<String>) {
        if path == self.attachment_root {
            return;
        }

        match fs::remove_dir(path) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => errors.push(format!("删除空附件目录失败 {}: {error}", path.display())),
        }
    }
}

fn cleanup_new_guidance_attachment_files(service: &StorageService, paths: &[PathBuf]) {
    let mut errors = Vec::new();
    for path in paths {
        match fs::remove_file(path) {
            Ok(()) => service.prune_empty_attachment_dirs(path.parent(), &mut errors),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }
}
