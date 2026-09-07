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
    pub fn prepare_conversation_turn_rewrite_attachments(
        &self,
        conversation_id: &str,
        message_id: &str,
        project_id: Option<&str>,
        attachments: &[AgentInputAttachment],
        created_at: i64,
    ) -> Result<PreparedConversationTurnRewriteAttachments, String> {
        if attachments.is_empty() {
            return Ok(PreparedConversationTurnRewriteAttachments {
                records: Vec::new(),
                paths_created_by_this_process: Vec::new(),
            });
        }
        fs::create_dir_all(&self.attachment_root)
            .map_err(|error| format!("创建附件库目录失败：{error}"))?;
        let connection = self.state.connection()?;
        ensure_conversation_exists(&connection, conversation_id)?;
        ensure_project_reference_exists(&connection, project_id)?;
        for attachment in attachments {
            if attachment_repository::get_attachment(&connection, &attachment.id)
                .map_err(storage_error)?
                .is_some()
            {
                return Err(format!("编辑重发附件 id 已存在：{}", attachment.id));
            }
        }
        drop(connection);
        let mut staged = Vec::with_capacity(attachments.len());
        for attachment in attachments {
            let bytes = input_attachment_bytes(attachment)?;
            let storage_rel_path = attachment_storage_rel_path(
                conversation_id,
                message_id,
                &attachment.id,
                &attachment.name,
            );
            let storage_path = self.attachment_root.join(&storage_rel_path);
            staged.push((
                AttachmentRecord {
                    id: attachment.id.clone(),
                    conversation_id: conversation_id.to_string(),
                    message_id: message_id.to_string(),
                    project_id: project_id.map(ToString::to_string),
                    kind: input_attachment_kind_label(attachment.kind).to_string(),
                    original_name: attachment.name.clone(),
                    mime_type: attachment.mime_type.clone(),
                    size_bytes: bytes.len() as u64,
                    storage_rel_path: slash_path(&storage_rel_path),
                    created_at,
                },
                bytes,
                storage_path,
            ));
        }
        let records = staged
            .iter()
            .map(|(record, _, _)| record.clone())
            .collect::<Vec<_>>();
        let mut created_paths = Vec::with_capacity(staged.len());
        for (_, bytes, storage_path) in &staged {
            let Some(parent) = storage_path.parent() else {
                self.discard_prepared_conversation_turn_rewrite_attachments(
                    PreparedConversationTurnRewriteAttachments {
                        records,
                        paths_created_by_this_process: created_paths,
                    },
                );
                return Err("附件存储路径无效。".to_string());
            };
            if let Err(error) = fs::create_dir_all(parent) {
                self.discard_prepared_conversation_turn_rewrite_attachments(
                    PreparedConversationTurnRewriteAttachments {
                        records,
                        paths_created_by_this_process: created_paths,
                    },
                );
                return Err(format!("创建附件目录失败：{error}"));
            }
            let created = match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(storage_path)
            {
                Ok(mut file) => {
                    use std::io::Write;
                    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                        let _ = fs::remove_file(storage_path);
                        self.discard_prepared_conversation_turn_rewrite_attachments(
                            PreparedConversationTurnRewriteAttachments {
                                records,
                                paths_created_by_this_process: created_paths,
                            },
                        );
                        return Err(format!("写入编辑重发附件失败：{error}"));
                    }
                    true
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let existing = match fs::read(storage_path) {
                        Ok(existing) => existing,
                        Err(read_error) => {
                            self.discard_prepared_conversation_turn_rewrite_attachments(
                                PreparedConversationTurnRewriteAttachments {
                                    records,
                                    paths_created_by_this_process: created_paths,
                                },
                            );
                            return Err(format!("读取并发编辑重发附件失败：{read_error}"));
                        }
                    };
                    if existing != *bytes {
                        self.discard_prepared_conversation_turn_rewrite_attachments(
                            PreparedConversationTurnRewriteAttachments {
                                records,
                                paths_created_by_this_process: created_paths,
                            },
                        );
                        return Err("编辑重发附件的确定性路径发生内容冲突。".to_string());
                    }
                    false
                }
                Err(error) => {
                    self.discard_prepared_conversation_turn_rewrite_attachments(
                        PreparedConversationTurnRewriteAttachments {
                            records,
                            paths_created_by_this_process: created_paths,
                        },
                    );
                    return Err(format!("写入编辑重发附件失败：{error}"));
                }
            };
            if created {
                created_paths.push(storage_path.clone());
            }
        }
        Ok(PreparedConversationTurnRewriteAttachments {
            records,
            paths_created_by_this_process: created_paths,
        })
    }

    pub fn discard_prepared_conversation_turn_rewrite_attachments(
        &self,
        prepared: PreparedConversationTurnRewriteAttachments,
    ) {
        cleanup_new_attachment_files(self, &prepared.paths_created_by_this_process);
    }

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
        drop(connection);

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
        drop(connection);

        let mut prepared = Vec::with_capacity(attachments.len());
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
            prepared.push((
                AttachmentRecord {
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
                },
                storage_path,
                bytes,
            ));
        }

        let mut newly_created_paths = Vec::new();
        for (_, storage_path, bytes) in &prepared {
            let parent = storage_path
                .parent()
                .ok_or_else(|| "附件存储路径无效。".to_string())?;
            fs::create_dir_all(parent).map_err(|error| format!("创建附件目录失败：{error}"))?;
            let created = match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(storage_path)
            {
                Ok(mut file) => {
                    use std::io::Write;
                    if let Err(error) = file.write_all(bytes) {
                        let _ = fs::remove_file(storage_path);
                        cleanup_new_attachment_files(self, &newly_created_paths);
                        return Err(format!("写入附件失败：{error}"));
                    }
                    true
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Err(error) = fs::write(storage_path, bytes) {
                        cleanup_new_attachment_files(self, &newly_created_paths);
                        return Err(format!("写入附件失败：{error}"));
                    }
                    false
                }
                Err(error) => {
                    cleanup_new_attachment_files(self, &newly_created_paths);
                    return Err(format!("写入附件失败：{error}"));
                }
            };
            if created {
                newly_created_paths.push(storage_path.clone());
            }
        }

        let database_result = (|| -> Result<(), String> {
            let mut connection = self.state.connection()?;
            let transaction = connection.transaction().map_err(storage_error)?;
            ensure_conversation_exists(&transaction, conversation_id)?;
            ensure_project_reference_exists(&transaction, project_id)?;
            for (record, _, _) in &prepared {
                attachment_repository::save_attachment(&transaction, record)
                    .map_err(storage_error)?;
            }
            transaction.commit().map_err(storage_error)
        })();
        if database_result.is_err() {
            cleanup_new_attachment_files(self, &newly_created_paths);
        }
        database_result
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
        let connection = self.state.connection()?;
        ensure_conversation_exists(&connection, &record.conversation_id)?;
        ensure_project_reference_exists(&connection, project_id)?;
        for attachment in attachments {
            if attachment_repository::get_attachment(&connection, &attachment.id)
                .map_err(storage_error)?
                .is_some()
            {
                return Err(format!("附件 id 已存在：{}", attachment.id));
            }
        }
        drop(connection);

        let mut prepared = Vec::with_capacity(attachments.len());
        for attachment in attachments {
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
                cleanup_new_attachment_files(self, &written_paths);
                return Err(format!("写入引导附件失败：{error}"));
            }
            written_paths.push(storage_path.clone());
        }

        let database_result = (|| -> Result<AgentRunGuidanceStoreOutcome, String> {
            let mut connection = self.state.connection()?;
            let transaction = connection.transaction().map_err(storage_error)?;
            ensure_conversation_exists(&transaction, &record.conversation_id)?;
            ensure_project_reference_exists(&transaction, project_id)?;
            for (attachment, _, _) in &prepared {
                if attachment_repository::get_attachment(&transaction, &attachment.id)
                    .map_err(storage_error)?
                    .is_some()
                {
                    return Err(format!("附件 id 已存在：{}", attachment.id));
                }
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
            cleanup_new_attachment_files(self, &written_paths);
        }
        database_result
    }

    pub fn load_input_attachments(
        &self,
        attachment_ids: &[String],
    ) -> Result<Vec<AgentInputAttachment>, String> {
        let connection = self.state.connection()?;
        let records = attachment_ids
            .iter()
            .map(|attachment_id| {
                attachment_repository::get_attachment(&connection, attachment_id)
                    .map_err(storage_error)?
                    .ok_or_else(|| format!("附件不存在：{attachment_id}"))
            })
            .collect::<Result<Vec<_>, String>>()?;
        drop(connection);

        let mut attachments = Vec::with_capacity(records.len());
        for attachment in records {
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

    /// Resolves immutable model-history image references within their owning conversation.
    /// References carry no read authority: deleted/superseded messages, foreign attachments,
    /// MIME drift, missing files and changed bytes all fail closed before model projection.
    pub fn load_context_image_attachments(
        &self,
        conversation_id: &str,
        refs: &[crate::ConversationContextImageRef],
    ) -> Result<Vec<AgentInputAttachment>, String> {
        use sha2::{Digest, Sha256};
        if refs.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.state.connection()?;
        let superseded = conversation_turn_rewrite_repository::superseded_message_ids(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)?;
        let mut seen = HashMap::new();
        let mut records = Vec::new();
        for reference in refs {
            reference.validate()?;
            if let Some(previous) = seen.insert(reference.attachment_id.clone(), reference) {
                if previous != reference {
                    return Err("context_image_reference_conflict: image identity changed".into());
                }
                continue;
            }
            let record =
                attachment_repository::get_attachment(&connection, &reference.attachment_id)
                    .map_err(storage_error)?
                    .ok_or_else(|| {
                        "context_image_unavailable: attachment is missing".to_string()
                    })?;
            if record.conversation_id != conversation_id
                || superseded.contains(&record.message_id)
                || record.kind != "image"
                || record.mime_type.as_deref() != Some(reference.mime_type.as_str())
            {
                return Err(
                    "context_image_scope_mismatch: image is outside visible history".into(),
                );
            }
            let visible: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1 AND conversation_id = ?2)",
                    rusqlite::params![record.message_id, conversation_id],
                    |row| row.get(0),
                )
                .map_err(storage_error)?;
            if !visible {
                return Err("context_image_unavailable: owner message is missing".into());
            }
            records.push((record, reference));
        }
        drop(connection);
        records
            .into_iter()
            .map(|(record, reference)| {
                let path = safe_existing_attachment_storage_path(
                    &self.attachment_root,
                    &record.storage_rel_path,
                )
                .ok_or_else(|| "context_image_unavailable: image bytes are missing".to_string())?;
                let bytes = fs::read(path).map_err(|_| {
                    "context_image_unavailable: image bytes cannot be read".to_string()
                })?;
                if bytes.len() as u64 != record.size_bytes
                    || format!("sha256:{:x}", Sha256::digest(&bytes)) != reference.sha256
                {
                    return Err("context_image_integrity_mismatch: image bytes changed".into());
                }
                Ok(AgentInputAttachment {
                    id: record.id,
                    kind: AgentInputAttachmentKind::Image,
                    name: record.original_name,
                    mime_type: record.mime_type,
                    size_bytes: record.size_bytes,
                    encoding: AgentInputAttachmentEncoding::Base64,
                    data: base64::engine::general_purpose::STANDARD.encode(bytes),
                    truncated: None,
                })
            })
            .collect()
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
        let agent_tree_scope = crate::storage::agent_tree_resource_scope::for_conversation(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)?;
        let project_attachments =
            attachment_repository::list_shared_attachments_for_library_excluding_conversation(
                &connection,
                project_id,
                agent_tree_scope.as_ref(),
                conversation_id,
            )
            .map_err(storage_error)?
            .into_iter()
            .map(agent_attachment_reference)
            .collect::<Vec<_>>();

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
    ) -> Result<Vec<AttachmentRecord>, String> {
        let mut preview_attachments = Vec::new();
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
                if image_preview_mime_type(&attachment).is_some() {
                    preview_attachments.push(attachment.clone());
                }
                attachments_by_message_id
                    .entry(attachment.message_id.clone())
                    .or_default()
                    .push(Self::chat_message_attachment_record(attachment));
            }

            for message in &mut conversation.messages {
                if let Some(attachments) = attachments_by_message_id.remove(&message.id) {
                    message.attachments = attachments;
                }
            }
        }

        Ok(preview_attachments)
    }

    pub(super) fn chat_message_attachment_record(
        attachment: AttachmentRecord,
    ) -> ChatMessageAttachmentRecord {
        let preview_mime_type = image_preview_mime_type(&attachment);

        ChatMessageAttachmentRecord {
            id: attachment.id,
            kind: attachment.kind,
            name: attachment.original_name,
            mime_type: attachment.mime_type,
            size_bytes: attachment.size_bytes,
            preview_data: None,
            preview_mime_type,
            created_at: attachment.created_at,
        }
    }

    /// Completes eager preview hydration after the caller releases its SQLite connection guard.
    pub(super) fn hydrate_message_attachment_previews(
        &self,
        conversations: &mut [ChatConversationRecord],
        preview_attachments: Vec<AttachmentRecord>,
    ) {
        let mut preview_data_by_id = preview_attachments
            .into_iter()
            .filter_map(|attachment| {
                self.read_attachment_preview_data(&attachment)
                    .map(|preview_data| (attachment.id, preview_data))
            })
            .collect::<HashMap<_, _>>();
        if preview_data_by_id.is_empty() {
            return;
        }

        for conversation in conversations {
            for message in &mut conversation.messages {
                for attachment in &mut message.attachments {
                    if let Some(preview_data) = preview_data_by_id.remove(&attachment.id) {
                        attachment.preview_data = Some(preview_data);
                    }
                }
            }
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

    pub(super) fn cleanup_orphan_attachment_files(&self) -> Result<(), String> {
        if !self.attachment_root.exists() {
            return Ok(());
        }

        let referenced_paths = {
            let connection = self.state.connection()?;
            attachment_repository::list_attachment_storage_rel_paths(&connection)
                .map_err(storage_error)?
                .into_iter()
                .collect::<HashSet<_>>()
        };
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

fn cleanup_new_attachment_files(service: &StorageService, paths: &[PathBuf]) {
    let referenced_paths = {
        let Ok(connection) = service.state.connection() else {
            return;
        };
        let Ok(referenced_paths) =
            attachment_repository::list_attachment_storage_rel_paths(&connection)
        else {
            return;
        };
        referenced_paths.into_iter().collect::<HashSet<_>>()
    };
    let mut errors = Vec::new();
    for path in paths {
        if orphan_scan_relative_path(&service.attachment_root, path)
            .is_some_and(|relative_path| referenced_paths.contains(&relative_path))
        {
            continue;
        }
        match fs::remove_file(path) {
            Ok(()) => service.prune_empty_attachment_dirs(path.parent(), &mut errors),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }
}
