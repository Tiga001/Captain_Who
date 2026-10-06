use super::*;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;

const MAX_HISTORY_SNAPSHOTS: usize = 32;
const MAX_HISTORY_SNAPSHOT_BYTES: usize = 128 * 1024 * 1024;

/// Opaque evidence for exactly one persisted history view, never a replacement for current
/// permissions, Provider state, project instructions or workflow input admission.
#[derive(Debug, Clone)]
pub struct ConversationHistoryVersion {
    database_instance: Arc<()>,
    conversation_id: String,
    digest: String,
}

impl PartialEq for ConversationHistoryVersion {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.database_instance, &other.database_instance)
            && self.conversation_id == other.conversation_id
            && self.digest == other.digest
    }
}
impl Eq for ConversationHistoryVersion {}
impl std::hash::Hash for ConversationHistoryVersion {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&Arc::as_ptr(&self.database_instance), state);
        std::hash::Hash::hash(&self.conversation_id, state);
        std::hash::Hash::hash(&self.digest, state);
    }
}

/// Fresh admission metadata with immutable, shared persisted message content.
#[derive(Debug, Clone)]
pub struct SharedHistoryConversation {
    metadata: ChatConversationMetaRecord,
    pub messages: Arc<Vec<ChatMessageRecord>>,
}

impl std::ops::Deref for SharedHistoryConversation {
    type Target = ChatConversationMetaRecord;
    fn deref(&self) -> &Self::Target {
        &self.metadata
    }
}
impl SharedHistoryConversation {
    /// Materializes the owned record only when a caller needs to append a new turn.
    pub fn to_record(&self) -> ChatConversationRecord {
        ChatConversationRecord {
            id: self.id.clone(),
            project_id: self.project_id.clone(),
            model_id: self.model_id.clone(),
            title: self.title.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            pinned_at: self.pinned_at,
            archived_at: self.archived_at,
            unread_at: self.unread_at,
            messages: self.messages.as_ref().clone(),
        }
    }
}
impl From<ChatConversationRecord> for SharedHistoryConversation {
    fn from(record: ChatConversationRecord) -> Self {
        Self {
            metadata: ChatConversationMetaRecord {
                id: record.id,
                project_id: record.project_id,
                model_id: record.model_id,
                title: record.title,
                created_at: record.created_at,
                updated_at: record.updated_at,
                pinned_at: record.pinned_at,
                archived_at: record.archived_at,
                unread_at: record.unread_at,
            },
            messages: Arc::new(record.messages),
        }
    }
}

#[derive(Debug)]
pub struct ConversationHistoryPayload {
    pub traces: Vec<ConversationTurnTrace>,
    pub model_context_logs: Vec<ConversationModelContextLog>,
    pub compaction_summary: Option<ContextCompactionSummary>,
}

#[derive(Debug, Clone)]
pub struct ConversationHistorySnapshot {
    pub conversation: SharedHistoryConversation,
    pub conversation_revision: i64,
    presentation_revision: i64,
    ui_revision: i64,
    pub payload: Arc<ConversationHistoryPayload>,
    pub version: ConversationHistoryVersion,
    pub estimated_bytes: usize,
}
impl std::ops::Deref for ConversationHistorySnapshot {
    type Target = ConversationHistoryPayload;
    fn deref(&self) -> &Self::Target {
        &self.payload
    }
}
impl ConversationHistorySnapshot {
    fn with_current_presentation(
        self: &Arc<Self>,
        connection: &Connection,
        conversation_revision: i64,
    ) -> Result<Arc<Self>, String> {
        let (presentation_revision, ui_revision) =
            message_presentation_revisions(connection, &self.conversation.id)?;
        if self.conversation_revision == conversation_revision
            && self.presentation_revision == presentation_revision
            && self.ui_revision == ui_revision
        {
            return Ok(self.clone());
        }
        let mut refreshed = self.as_ref().clone();
        if self.conversation_revision != conversation_revision {
            let metadata =
                chat_repository::get_conversation_meta(connection, &self.conversation.id)
                    .map_err(storage_error)?
                    .ok_or_else(|| "历史上下文的会话已不存在。".to_string())?;
            // Refresh the small metadata wrapper without rescanning decoded history payloads.
            refreshed.estimated_bytes = refreshed
                .estimated_bytes
                .saturating_sub(serialized_size(&self.conversation.metadata)?.saturating_mul(2))
                .saturating_add(serialized_size(&metadata)?.saturating_mul(2));
            refreshed.conversation.metadata = metadata;
            refreshed.conversation_revision = conversation_revision;
        }
        if self.presentation_revision != presentation_revision {
            // The indexed query reads only changed agent JSON, without loading trace/model
            // payloads. Copy-on-write messages keep previously returned snapshots immutable.
            let mut statement = connection
                .prepare(
                    "SELECT id, agent_run_json FROM messages
                 WHERE conversation_id = ?1 AND presentation_revision > ?2",
                )
                .map_err(storage_error)?;
            let changes = statement
                .query_map(
                    rusqlite::params![self.conversation.id, self.presentation_revision],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .map_err(storage_error)?
                .collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()
                .map_err(storage_error)?;
            if !changes.is_empty() {
                let messages = Arc::make_mut(&mut refreshed.conversation.messages);
                for message in messages {
                    if let Some(agent_run_json) = changes.get(&message.id) {
                        refreshed.estimated_bytes = refreshed
                            .estimated_bytes
                            .saturating_sub(
                                serialized_size(&message.agent_run_json)?.saturating_mul(2),
                            )
                            .saturating_add(serialized_size(agent_run_json)?.saturating_mul(2));
                        message.agent_run_json = agent_run_json.clone();
                    }
                }
            }
            refreshed.presentation_revision = presentation_revision;
        }
        if self.ui_revision != ui_revision {
            // UI state is a separate overlay, including for immutable fork/snapshot messages.
            // Read its small rows only when its watermark changes; missing rows mean deletion.
            let mut statement = connection
                .prepare(
                    "SELECT ui.message_id, ui.ui_state_json FROM chat_message_ui_states ui
                 JOIN messages message ON message.id = ui.message_id
                 WHERE message.conversation_id = ?1",
                )
                .map_err(storage_error)?;
            let states = statement
                .query_map([&self.conversation.id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(storage_error)?
                .collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()
                .map_err(storage_error)?;
            for message in Arc::make_mut(&mut refreshed.conversation.messages) {
                let ui_state_json = states.get(&message.id).cloned();
                refreshed.estimated_bytes = refreshed
                    .estimated_bytes
                    .saturating_sub(serialized_size(&message.ui_state_json)?.saturating_mul(2))
                    .saturating_add(serialized_size(&ui_state_json)?.saturating_mul(2));
                message.ui_state_json = ui_state_json;
            }
            refreshed.ui_revision = ui_revision;
        }
        Ok(Arc::new(refreshed))
    }
}

#[cfg(any(test, debug_assertions))]
#[derive(Debug, Clone, Copy, Default)]
pub struct ConversationHistorySnapshotDiagnostics {
    pub full_loads: u64,
    pub trace_decodes: u64,
    pub model_context_decodes: u64,
}

#[derive(Default)]
pub(super) struct ConversationHistorySnapshotCache {
    entries: VecDeque<Arc<ConversationHistorySnapshot>>,
    bytes: usize,
    #[cfg(any(test, debug_assertions))]
    diagnostics: std::collections::HashMap<String, ConversationHistorySnapshotDiagnostics>,
}

impl ConversationHistorySnapshotCache {
    fn get(
        &mut self,
        version: &ConversationHistoryVersion,
    ) -> Option<Arc<ConversationHistorySnapshot>> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.version.conversation_id == version.conversation_id)?;
        let entry = self.entries.remove(index)?;
        if entry.version != *version {
            self.bytes = self.bytes.saturating_sub(entry.estimated_bytes);
            return None;
        }
        self.entries.push_back(entry.clone());
        Some(entry)
    }

    fn insert(&mut self, snapshot: Arc<ConversationHistorySnapshot>) -> bool {
        self.remove(&snapshot.version.conversation_id);
        if snapshot.estimated_bytes > MAX_HISTORY_SNAPSHOT_BYTES {
            return false;
        }
        while self.entries.len() >= MAX_HISTORY_SNAPSHOTS
            || self.bytes.saturating_add(snapshot.estimated_bytes) > MAX_HISTORY_SNAPSHOT_BYTES
        {
            if let Some(old) = self.entries.pop_front() {
                self.bytes = self.bytes.saturating_sub(old.estimated_bytes);
            } else {
                break;
            }
        }
        self.bytes += snapshot.estimated_bytes;
        self.entries.push_back(snapshot);
        true
    }

    fn remove(&mut self, conversation_id: &str) {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.version.conversation_id == conversation_id)
        {
            if let Some(entry) = self.entries.remove(index) {
                self.bytes = self.bytes.saturating_sub(entry.estimated_bytes);
            }
        }
    }
}

/// Every large trace/model payload has a trigger-owned journal revision. Small side journals
/// without such revisions are included directly; notably summary contents cannot change behind
/// an unchanged head ID. Message history uses its own trigger-owned revision: presentation
/// metadata and unrelated writes through any database connection do not invalidate history.
/// The separate conversation revision is returned only for admission's metadata CAS.
pub(super) fn history_version_in_connection(
    connection: &Connection,
    conversation_id: &str,
    database_instance: &Arc<()>,
) -> Result<Option<(ConversationHistoryVersion, i64)>, String> {
    let revision = connection
        .query_row(
            "SELECT revision FROM conversations WHERE id = ?1",
            [conversation_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(storage_error)?;
    let Some(revision) = revision else {
        return Ok(None);
    };
    let missing_journal: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversation_turn_traces trace LEFT JOIN conversation_trace_journal_revisions journal ON journal.assistant_message_id = trace.assistant_message_id WHERE trace.conversation_id = ?1 AND journal.assistant_message_id IS NULL)",
        [conversation_id], |row| row.get(0),
    ).map_err(storage_error)?;
    if missing_journal {
        return Err("历史上下文缺少可信的日志版本记录。".into());
    }
    let message_version = connection.query_row(
        "SELECT epoch, revision FROM conversation_message_history_revisions WHERE conversation_id = ?1",
        [conversation_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    ).optional().map_err(storage_error)?
        .ok_or_else(|| "历史上下文缺少可信的消息版本记录。".to_string())?;
    let mut digest = Sha256::new();
    digest.update((message_version.0.len() as u64).to_le_bytes());
    digest.update(message_version.0.as_bytes());
    digest.update(message_version.1.to_le_bytes());
    for sql in [
        "SELECT json_array(trace.assistant_message_id, journal.epoch, journal.revision) FROM conversation_turn_traces trace LEFT JOIN conversation_trace_journal_revisions journal ON journal.assistant_message_id = trace.assistant_message_id WHERE trace.conversation_id = ?1 ORDER BY trace.assistant_message_id",
        "SELECT json_array(head.summary_id, summary.id, summary.conversation_id, summary.schema_version, summary.source_revision, summary.previous_summary_id, summary.covered_through_kind, summary.covered_through_message_id, summary.covered_through_trace_sequence, summary.content, summary.continuity_schema_version, summary.continuity_json, summary.generation_kind, summary.generation_model, summary.source_input_tokens, summary.summary_input_tokens, summary.continuity_input_tokens, summary.uncovered_tail_input_tokens, summary.replacement_input_tokens, summary.created_at) FROM conversation_context_compaction_heads head LEFT JOIN context_compaction_summaries summary ON summary.id = head.summary_id WHERE head.conversation_id = ?1",
        "SELECT json_array(id, message_id, project_id, kind, original_name, mime_type, size_bytes, storage_rel_path, created_at, pasted_text_json) FROM attachments WHERE conversation_id = ?1 ORDER BY id",
        "SELECT json_array(ownership.attachment_id) FROM agent_run_guidance_attachments ownership JOIN attachments attachment ON attachment.id = ownership.attachment_id WHERE attachment.conversation_id = ?1 ORDER BY ownership.attachment_id",
        "SELECT json_array(source_user_message_id, source_assistant_message_id) FROM conversation_turn_rewrites WHERE conversation_id = ?1 ORDER BY source_assistant_message_id",
        "SELECT json_array(projection.message_id, projection.request_id, projection.response_id, projection.content_json) FROM human_interaction_message_projections projection JOIN messages message ON message.id = projection.message_id WHERE message.conversation_id = ?1 ORDER BY projection.message_id",
        "SELECT json_array(lifecycle.event_id, lifecycle.recorded_at, lifecycle.event_json, lifecycle.created_at, lifecycle.phase, lifecycle.session_id) FROM agent_command_session_lifecycle_events lifecycle JOIN conversation_turn_traces trace ON trace.assistant_message_id = lifecycle.assistant_message_id WHERE trace.conversation_id = ?1 AND lifecycle.trace_sequence IS NULL ORDER BY lifecycle.event_id",
        "SELECT DISTINCT json_array(message.id) FROM workflow_mail_inputs input JOIN messages message ON message.id = json_extract(input.input_json, '$.deliveryId') WHERE input.conversation_id = ?1 AND message.conversation_id = ?1 AND NOT EXISTS (SELECT 1 FROM conversation_turn_rewrites rewrite WHERE rewrite.conversation_id = message.conversation_id AND (rewrite.source_user_message_id = message.id OR rewrite.source_assistant_message_id = message.id)) ORDER BY message.id",
        "SELECT json_array(archive_ref) FROM conversation_history_blobs WHERE conversation_id = ?1 ORDER BY archive_ref",
        "SELECT json_array(schema_version, reason, source_conversation_id, source_message_id, created_at, resolved_summary_id, resolved_at) FROM conversation_context_adaptation_requirements WHERE conversation_id = ?1",
        "SELECT json_array(EXISTS(SELECT 1 FROM provider_continuations WHERE conversation_id = ?1 AND state = 'released'))",

        "SELECT json_array(mailbox.projection_message_id, receipt_item.delivery_path, receipt.sampling_bound_at) FROM agent_mailbox_messages mailbox JOIN agent_model_batch_receipt_items receipt_item ON receipt_item.message_id = mailbox.message_id JOIN agent_model_batch_receipts receipt ON receipt.receipt_id = receipt_item.receipt_id JOIN messages message ON message.id = mailbox.projection_message_id WHERE message.conversation_id = ?1 ORDER BY mailbox.message_id, receipt_item.receipt_id",
    ] {
        let mut statement = connection.prepare(sql).map_err(storage_error)?;
        let rows = statement.query_map([conversation_id], |row| row.get::<_, String>(0)).map_err(storage_error)?;
        digest.update(sql.as_bytes());
        for row in rows { let row = row.map_err(storage_error)?; digest.update((row.len() as u64).to_le_bytes()); digest.update(row.as_bytes()); }
    }
    Ok(Some((
        ConversationHistoryVersion {
            database_instance: database_instance.clone(),
            conversation_id: conversation_id.into(),
            digest: format!("{:x}", digest.finalize()),
        },
        revision,
    )))
}

impl StorageService {
    #[cfg(any(test, debug_assertions))]
    pub fn history_snapshot_diagnostics(
        &self,
        conversation_id: &str,
    ) -> ConversationHistorySnapshotDiagnostics {
        self.history_snapshot_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .diagnostics
            .get(conversation_id)
            .copied()
            .unwrap_or_default()
    }

    #[cfg(any(test, debug_assertions))]
    fn record_history_snapshot_work(
        &self,
        conversation_id: &str,
        update: impl FnOnce(&mut ConversationHistorySnapshotDiagnostics),
    ) -> Result<(), String> {
        let mut cache = self
            .history_snapshot_cache
            .lock()
            .map_err(|_| "历史上下文缓存状态不可用。".to_string())?;
        // Diagnostics are host-local and bounded independently of history payload eviction.
        if cache.diagnostics.len() >= 256 && !cache.diagnostics.contains_key(conversation_id) {
            cache.diagnostics.clear();
        }
        update(cache.diagnostics.entry(conversation_id.into()).or_default());
        Ok(())
    }

    pub fn is_conversation_history_snapshot_current(
        &self,
        version: &ConversationHistoryVersion,
    ) -> Result<bool, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let current = history_version_in_connection(
            &transaction,
            &version.conversation_id,
            &self.state.instance_identity,
        )?;
        transaction.commit().map_err(storage_error)?;
        Ok(current.is_some_and(|(current, _)| current == *version))
    }

    /// Returns one transactionally captured raw history. Active turns are readable but never
    /// cached. Decode and hash validation happen without holding the shared database mutex.
    pub fn load_conversation_history_snapshot(
        &self,
        conversation_id: &str,
    ) -> Result<Option<Arc<ConversationHistorySnapshot>>, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let Some((version, conversation_revision)) = history_version_in_connection(
            &transaction,
            conversation_id,
            &self.state.instance_identity,
        )?
        else {
            return Ok(None);
        };
        let (presentation_revision, ui_revision) =
            message_presentation_revisions(&transaction, conversation_id)?;
        let active = conversation_trace_repository::get_in_progress_turn_identity(
            &transaction,
            conversation_id,
        )
        .map_err(storage_error)?
        .is_some();
        if !active {
            let mut cache = self
                .history_snapshot_cache
                .lock()
                .map_err(|_| "历史上下文缓存状态不可用。".to_string())?;
            if let Some(snapshot) = cache.get(&version) {
                let snapshot =
                    snapshot.with_current_presentation(&transaction, conversation_revision)?;
                cache.insert(snapshot.clone());
                transaction.commit().map_err(storage_error)?;
                return Ok(Some(snapshot));
            }
        }
        #[cfg(any(test, debug_assertions))]
        self.record_history_snapshot_work(conversation_id, |metrics| metrics.full_loads += 1)?;
        let Some(mut conversation) =
            chat_repository::get_active_persisted_conversation(&transaction, conversation_id)
                .map_err(storage_error)?
        else {
            return Ok(None);
        };
        let preview_attachments =
            self.attach_message_attachments(&transaction, std::slice::from_mut(&mut conversation))?;
        let stored_traces =
            conversation_trace_repository::load_stored_trace_records_for_conversation(
                &transaction,
                conversation_id,
            )
            .map_err(storage_error)?;
        let stored_logs = conversation_model_context_repository::load_stored_logs_for_conversation(
            &transaction,
            conversation_id,
        )
        .map_err(storage_error)?;
        let summary_validation = context_compaction_repository::load_snapshot_summary_validation(
            &transaction,
            conversation_id,
        )
        .map_err(|error| error.to_string())?;
        let superseded = conversation_turn_rewrite_repository::superseded_message_ids(
            &transaction,
            conversation_id,
        )
        .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        drop(connection);
        #[cfg(any(test, debug_assertions))]
        self.record_history_snapshot_work(conversation_id, |metrics| metrics.trace_decodes += 1)?;
        let mut traces: Vec<_> = stored_traces
            .decode()
            .map_err(storage_error)?
            .into_iter()
            .map(|record| record.trace)
            .collect();
        let (compaction_summary, summary_valid) = summary_validation
            .validate(&traces)
            .map_err(|error| error.to_string())?;
        if !summary_valid {
            #[cfg(debug_assertions)]
            eprintln!("[历史上下文缓存] 摘要已失效：会话={}，原因=原始前缀或连续性引用变化，本次使用完整历史（不修改数据库）", conversation_id);
        }
        #[cfg(any(test, debug_assertions))]
        self.record_history_snapshot_work(conversation_id, |metrics| {
            metrics.model_context_decodes += 1
        })?;
        let mut model_context_logs = stored_logs.decode().map_err(storage_error)?;
        traces.retain(|trace| !superseded.contains(&trace.assistant_message_id));
        model_context_logs.retain(|log| !superseded.contains(&log.assistant_message_id));
        self.hydrate_message_attachment_previews(
            std::slice::from_mut(&mut conversation),
            preview_attachments,
        );
        let estimated_bytes = serialized_size(&(
            &conversation,
            &traces,
            &model_context_logs,
            &compaction_summary,
        ))?
        .saturating_mul(2);
        let mut snapshot = Arc::new(ConversationHistorySnapshot {
            conversation: conversation.into(),
            conversation_revision,
            presentation_revision,
            ui_revision,
            payload: Arc::new(ConversationHistoryPayload {
                traces,
                model_context_logs,
                compaction_summary,
            }),
            version,
            estimated_bytes,
        });
        // Never let a slow preparation overwrite newer committed history. The cache lock is
        // taken only while holding the short DB transaction, matching the hot lookup order.
        if !active {
            let mut connection = self.state.connection()?;
            let transaction = connection.transaction().map_err(storage_error)?;
            if let Some((_, revision)) = history_version_in_connection(
                &transaction,
                conversation_id,
                &self.state.instance_identity,
            )?
            .filter(|(version, _)| version == &snapshot.version)
            {
                // Presentation may change while the large bodies are decoded. Refresh raw
                // agent JSON and admission metadata together under the final read transaction.
                snapshot = snapshot.with_current_presentation(&transaction, revision)?;
                let _ = self
                    .history_snapshot_cache
                    .lock()
                    .map_err(|_| "历史上下文缓存状态不可用。".to_string())?
                    .insert(snapshot.clone());
            }
            transaction.commit().map_err(storage_error)?;
        }
        Ok(Some(snapshot))
    }
}

fn message_presentation_revisions(
    connection: &Connection,
    conversation_id: &str,
) -> Result<(i64, i64), String> {
    connection.query_row(
        "SELECT presentation_revision, ui_revision FROM conversation_message_history_revisions WHERE conversation_id = ?1",
        [conversation_id], |row| Ok((row.get(0)?, row.get(1)?)),
    ).map_err(storage_error)
}

fn serialized_size(value: &impl serde::Serialize) -> Result<usize, String> {
    struct Size(usize);
    impl std::io::Write for Size {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut size = Size(0);
    serde_json::to_writer(&mut size, value).map_err(|error| error.to_string())?;
    Ok(size.0)
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    fn snapshot(id: usize, bytes: usize) -> Arc<ConversationHistorySnapshot> {
        Arc::new(ConversationHistorySnapshot {
            conversation: ChatConversationRecord {
                id: id.to_string(),
                project_id: None,
                model_id: None,
                title: String::new(),
                messages: Vec::new(),
                created_at: 0,
                updated_at: 0,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            }
            .into(),
            conversation_revision: 0,
            presentation_revision: 0,
            ui_revision: 0,
            payload: Arc::new(ConversationHistoryPayload {
                traces: Vec::new(),
                model_context_logs: Vec::new(),
                compaction_summary: None,
            }),
            version: ConversationHistoryVersion {
                database_instance: {
                    static IDENTITY: std::sync::OnceLock<Arc<()>> = std::sync::OnceLock::new();
                    IDENTITY.get_or_init(|| Arc::new(())).clone()
                },
                conversation_id: id.to_string(),
                digest: "valid".into(),
            },
            estimated_bytes: bytes,
        })
    }

    #[test]
    fn history_snapshot_cache_limits_bytes_and_evicts_least_recently_used() {
        let mut cache = ConversationHistorySnapshotCache::default();
        for id in 0..MAX_HISTORY_SNAPSHOTS {
            cache.insert(snapshot(id, 1));
        }
        assert!(cache.get(&snapshot(0, 1).version).is_some());
        cache.insert(snapshot(MAX_HISTORY_SNAPSHOTS, 1));
        assert!(cache.get(&snapshot(1, 1).version).is_none());
        assert!(cache.get(&snapshot(0, 1).version).is_some());
        cache.insert(snapshot(100, MAX_HISTORY_SNAPSHOT_BYTES));
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.bytes, MAX_HISTORY_SNAPSHOT_BYTES);
        assert!(!cache.insert(snapshot(101, MAX_HISTORY_SNAPSHOT_BYTES + 1)));
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.bytes, MAX_HISTORY_SNAPSHOT_BYTES);
    }
}
