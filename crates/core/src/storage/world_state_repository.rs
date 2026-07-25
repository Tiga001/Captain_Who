//! Durable, backend-owned conversation World State journal.
//!
//! The repository intentionally stores the provider-neutral domain record as canonical JSON while
//! indexing the small amount of metadata needed for ordering, idempotency and exact rewind. The
//! domain reducer remains the authority for interpreting a full snapshot or diff.

use crate::world_state::{
    AnchoredWorldStateRecord, WorldStateDiff, WorldStateRecord, WorldStateRecordKind,
    WorldStateReducer, WorldStateSnapshot,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationWorldStateRecordKind {
    Full,
    Diff,
}

impl ConversationWorldStateRecordKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Diff => "diff",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "full" => Some(Self::Full),
            "diff" => Some(Self::Diff),
            _ => None,
        }
    }
}

fn record_kind_from_domain(kind: WorldStateRecordKind) -> ConversationWorldStateRecordKind {
    match kind {
        WorldStateRecordKind::Full => ConversationWorldStateRecordKind::Full,
        WorldStateRecordKind::Diff => ConversationWorldStateRecordKind::Diff,
    }
}

#[derive(Debug, Clone)]
pub struct ConversationWorldStateRecordWrite<'a> {
    pub conversation_id: &'a str,
    pub epoch_generation: u64,
    pub base_summary_id: Option<&'a str>,
    pub effective_before_message_id: Option<&'a str>,
    pub record: &'a WorldStateRecord,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
struct IndexedWorldStateRecordWrite<'a> {
    conversation_id: &'a str,
    schema_version: u32,
    epoch_id: &'a str,
    epoch_generation: u64,
    base_summary_id: Option<&'a str>,
    sequence: u64,
    record_kind: ConversationWorldStateRecordKind,
    base_revision: Option<&'a str>,
    result_revision: &'a str,
    effective_before_message_id: Option<&'a str>,
    record_json: String,
    created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredConversationWorldStateRecord {
    pub conversation_id: String,
    pub schema_version: u32,
    pub epoch_id: String,
    pub epoch_generation: u64,
    pub base_summary_id: Option<String>,
    pub sequence: u64,
    pub record_kind: ConversationWorldStateRecordKind,
    pub base_revision: Option<String>,
    pub result_revision: String,
    pub effective_before_message_id: Option<String>,
    pub record_json: String,
    pub created_at: i64,
}

impl StoredConversationWorldStateRecord {
    /// Decodes and validates the provider-neutral domain record, including every duplicated SQL
    /// index field. A metadata/JSON mismatch is treated as corruption rather than being repaired.
    pub fn decode_record(&self) -> Result<WorldStateRecord, ConversationWorldStateRepositoryError> {
        let record =
            serde_json::from_str::<WorldStateRecord>(&self.record_json).map_err(|error| {
                ConversationWorldStateRepositoryError::Corrupt(format!(
                    "epoch `{}` sequence {} 的 JSON 无法解码：{error}",
                    self.epoch_id, self.sequence
                ))
            })?;
        record.validate().map_err(|error| {
            ConversationWorldStateRepositoryError::Corrupt(format!(
                "epoch `{}` sequence {} 的 domain record 无效：{error}",
                self.epoch_id, self.sequence
            ))
        })?;
        if record.schema_version() != self.schema_version
            || record.epoch_id() != self.epoch_id
            || record.sequence() != self.sequence
            || record_kind_from_domain(record.kind()) != self.record_kind
            || record.base_revision() != self.base_revision.as_deref()
            || record.result_revision() != self.result_revision
        {
            return Err(ConversationWorldStateRepositoryError::Corrupt(format!(
                "epoch `{}` sequence {} 的索引字段与 canonical record JSON 不一致。",
                self.epoch_id, self.sequence
            )));
        }
        Ok(record)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConversationWorldStateJournalEntry {
    pub conversation_id: String,
    pub epoch_generation: u64,
    pub base_summary_id: Option<String>,
    pub effective_before_message_id: Option<String>,
    pub record: WorldStateRecord,
    pub created_at: i64,
}

impl ConversationWorldStateJournalEntry {
    pub fn anchored_record(&self) -> AnchoredWorldStateRecord {
        AnchoredWorldStateRecord {
            record: self.record.clone(),
            effective_before_message_id: self.effective_before_message_id.clone(),
        }
    }
}

impl TryFrom<StoredConversationWorldStateRecord> for ConversationWorldStateJournalEntry {
    type Error = ConversationWorldStateRepositoryError;

    fn try_from(stored: StoredConversationWorldStateRecord) -> Result<Self, Self::Error> {
        let record = stored.decode_record()?;
        Ok(Self {
            conversation_id: stored.conversation_id,
            epoch_generation: stored.epoch_generation,
            base_summary_id: stored.base_summary_id,
            effective_before_message_id: stored.effective_before_message_id,
            record,
            created_at: stored.created_at,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationWorldStateAppendOutcome {
    Inserted,
    Idempotent,
}

#[derive(Debug, Clone)]
pub struct ConversationWorldStateRebaseRequest<'a> {
    pub conversation_id: &'a str,
    pub expected_source_epoch_id: &'a str,
    pub expected_source_revision: &'a str,
    /// Message containing the compaction cursor.
    ///
    /// A message cursor covers that message. A trace cursor covers an item inside its assistant
    /// message; World State anchors are message-granular and effective before the whole message,
    /// so both cursor kinds intentionally use the same message boundary here.
    pub covered_through_message_id: &'a str,
    pub new_epoch_id: &'a str,
    pub base_summary_id: &'a str,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConversationWorldStateRebaseOutcome {
    pub append_outcome: ConversationWorldStateAppendOutcome,
    pub full_snapshot: WorldStateSnapshot,
    pub epoch_generation: u64,
}

#[derive(Debug, Clone)]
struct RebasedWorldStateRecord {
    record: WorldStateRecord,
    effective_before_message_id: Option<String>,
    created_at: i64,
}

#[derive(Debug, Clone)]
struct ConversationWorldStateRebasePlan {
    source_epoch_generation: u64,
    records: Vec<RebasedWorldStateRecord>,
}

#[derive(Debug)]
pub enum ConversationWorldStateRepositoryError {
    Database(rusqlite::Error),
    Invalid(String),
    Conflict(String),
    Corrupt(String),
}

impl Display for ConversationWorldStateRepositoryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "本地数据库操作失败：{error}"),
            Self::Invalid(message) => write!(formatter, "Conversation World State 无效：{message}"),
            Self::Conflict(message) => {
                write!(formatter, "Conversation World State 写入冲突：{message}")
            }
            Self::Corrupt(message) => {
                write!(formatter, "Conversation World State 存储已损坏：{message}")
            }
        }
    }
}

impl Error for ConversationWorldStateRepositoryError {}

impl From<rusqlite::Error> for ConversationWorldStateRepositoryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

/// Appends one immutable record in its own transaction.
///
/// Retrying an identical `(conversation, epoch, sequence)` write is idempotent. Reusing that key
/// with different semantic data is rejected instead of silently replacing history.
pub fn append_record(
    connection: &mut Connection,
    record: &ConversationWorldStateRecordWrite<'_>,
) -> Result<ConversationWorldStateAppendOutcome, ConversationWorldStateRepositoryError> {
    let transaction = connection.transaction()?;
    let outcome = append_record_in_connection(&transaction, record)?;
    transaction.commit()?;
    Ok(outcome)
}

/// Appends one record inside a caller-owned transaction.
///
/// Compaction uses this entry point to make a newly folded full snapshot visible atomically with
/// the summary that establishes its epoch boundary.
pub(crate) fn append_record_in_connection(
    connection: &Connection,
    record: &ConversationWorldStateRecordWrite<'_>,
) -> Result<ConversationWorldStateAppendOutcome, ConversationWorldStateRepositoryError> {
    record.record.validate().map_err(|error| {
        ConversationWorldStateRepositoryError::Invalid(format!("domain record 校验失败：{error}"))
    })?;
    let indexed = IndexedWorldStateRecordWrite {
        conversation_id: record.conversation_id,
        schema_version: record.record.schema_version(),
        epoch_id: record.record.epoch_id(),
        epoch_generation: record.epoch_generation,
        base_summary_id: record.base_summary_id,
        sequence: record.record.sequence(),
        record_kind: record_kind_from_domain(record.record.kind()),
        base_revision: record.record.base_revision(),
        result_revision: record.record.result_revision(),
        effective_before_message_id: record.effective_before_message_id,
        record_json: record.record.canonical_json(),
        created_at: record.created_at,
    };
    append_indexed_record_in_connection(connection, &indexed)
}

fn append_indexed_record_in_connection(
    connection: &Connection,
    record: &IndexedWorldStateRecordWrite<'_>,
) -> Result<ConversationWorldStateAppendOutcome, ConversationWorldStateRepositoryError> {
    validate_write(record)?;

    if let Some(existing) = get_record(
        connection,
        record.conversation_id,
        record.epoch_id,
        record.sequence,
    )? {
        if semantically_matches(record, &existing) {
            return Ok(ConversationWorldStateAppendOutcome::Idempotent);
        }
        return Err(ConversationWorldStateRepositoryError::Conflict(format!(
            "epoch `{}` sequence {} 已被不同记录占用。",
            record.epoch_id, record.sequence
        )));
    }

    let conversation_exists = connection
        .query_row(
            "SELECT 1 FROM conversations WHERE id = ?1",
            [record.conversation_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !conversation_exists {
        return Err(ConversationWorldStateRepositoryError::Invalid(format!(
            "会话 `{}` 不存在。",
            record.conversation_id
        )));
    }

    let active_epoch = get_active_epoch(connection, record.conversation_id)?;
    let requested_epoch = get_epoch(connection, record.conversation_id, record.epoch_id)?;
    match requested_epoch {
        Some(epoch) => {
            if active_epoch
                .as_ref()
                .is_some_and(|active| active.epoch_id != epoch.epoch_id)
            {
                return Err(ConversationWorldStateRepositoryError::Conflict(
                    "不能向已经失活的 World State epoch 追加记录。".to_string(),
                ));
            }
            if epoch.generation != record.epoch_generation
                || epoch.base_summary_id.as_deref() != record.base_summary_id
            {
                return Err(ConversationWorldStateRepositoryError::Conflict(
                    "现有 World State epoch 的 generation 或压缩边界不一致。".to_string(),
                ));
            }
            validate_next_record(connection, record)?;
            validate_exact_next_diff(connection, record)?;
        }
        None => {
            validate_new_epoch(record, active_epoch.as_ref())?;
            connection.execute(
                "INSERT INTO conversation_world_state_epochs (
                    conversation_id, epoch_id, generation, base_summary_id, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    record.conversation_id,
                    record.epoch_id,
                    sqlite_u64(record.epoch_generation, "epoch generation")?,
                    record.base_summary_id,
                    record.created_at,
                ],
            )?;
        }
    }

    connection.execute(
        "INSERT INTO conversation_world_state_records (
            conversation_id,
            schema_version,
            epoch_id,
            sequence,
            record_kind,
            base_revision,
            result_revision,
            effective_before_message_id,
            record_json,
            created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            record.conversation_id,
            i64::from(record.schema_version),
            record.epoch_id,
            sqlite_u64(record.sequence, "record sequence")?,
            record.record_kind.as_str(),
            record.base_revision,
            record.result_revision,
            record.effective_before_message_id,
            &record.record_json,
            record.created_at,
        ],
    )?;

    Ok(ConversationWorldStateAppendOutcome::Inserted)
}

pub fn list_active_records(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<StoredConversationWorldStateRecord>, ConversationWorldStateRepositoryError> {
    let Some(epoch) = get_active_epoch(connection, conversation_id)? else {
        return Ok(Vec::new());
    };
    list_records_for_epoch(connection, conversation_id, &epoch.epoch_id)
}

pub fn list_active_journal_entries(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<ConversationWorldStateJournalEntry>, ConversationWorldStateRepositoryError> {
    list_active_records(connection, conversation_id)?
        .into_iter()
        .map(ConversationWorldStateJournalEntry::try_from)
        .collect()
}

pub fn list_records_for_epoch(
    connection: &Connection,
    conversation_id: &str,
    epoch_id: &str,
) -> Result<Vec<StoredConversationWorldStateRecord>, ConversationWorldStateRepositoryError> {
    let mut statement = connection.prepare(
        "SELECT
            record.conversation_id,
            record.schema_version,
            record.epoch_id,
            epoch.generation,
            epoch.base_summary_id,
            record.sequence,
            record.record_kind,
            record.base_revision,
            record.result_revision,
            record.effective_before_message_id,
            record.record_json,
            record.created_at
         FROM conversation_world_state_records AS record
         INNER JOIN conversation_world_state_epochs AS epoch
           ON epoch.conversation_id = record.conversation_id
          AND epoch.epoch_id = record.epoch_id
         WHERE record.conversation_id = ?1 AND record.epoch_id = ?2
         ORDER BY record.sequence ASC, record.journal_position ASC",
    )?;
    let rows = statement
        .query_map(params![conversation_id, epoch_id], stored_record_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(validate_stored_record).collect()
}

pub fn get_active_head(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<StoredConversationWorldStateRecord>, ConversationWorldStateRepositoryError> {
    let Some(active_epoch) = get_active_epoch(connection, conversation_id)? else {
        return Ok(None);
    };
    let row = connection
        .query_row(
            "SELECT
                record.conversation_id,
                record.schema_version,
                record.epoch_id,
                epoch.generation,
                epoch.base_summary_id,
                record.sequence,
                record.record_kind,
                record.base_revision,
                record.result_revision,
                record.effective_before_message_id,
                record.record_json,
                record.created_at
             FROM conversation_world_state_epochs AS epoch
             INNER JOIN conversation_world_state_records AS record
               ON record.conversation_id = epoch.conversation_id
              AND record.epoch_id = epoch.epoch_id
             WHERE epoch.conversation_id = ?1 AND epoch.epoch_id = ?2
             ORDER BY record.sequence DESC, record.journal_position DESC
             LIMIT 1",
            params![conversation_id, active_epoch.epoch_id],
            stored_record_from_row,
        )
        .optional()?;
    let Some(row) = row else {
        return Err(ConversationWorldStateRepositoryError::Corrupt(
            "active World State epoch 没有任何记录。".to_string(),
        ));
    };
    validate_stored_record(row).map(Some)
}

pub fn get_active_head_entry(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<ConversationWorldStateJournalEntry>, ConversationWorldStateRepositoryError> {
    get_active_head(connection, conversation_id)?
        .map(ConversationWorldStateJournalEntry::try_from)
        .transpose()
}

/// Folds the active epoch with the exact backend reducer.
///
/// This is deliberately not a semantic summary: every diff must form a valid revision and
/// sequence chain from the epoch's full snapshot.
pub fn fold_active_snapshot(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<WorldStateSnapshot>, ConversationWorldStateRepositoryError> {
    let entries = list_active_journal_entries(connection, conversation_id)?;
    let Some(first) = entries.first() else {
        return Ok(None);
    };
    let WorldStateRecord::Full(initial) = &first.record else {
        return Err(ConversationWorldStateRepositoryError::Corrupt(
            "active World State epoch 未以 full snapshot 开始。".to_string(),
        ));
    };
    if initial.sequence != 0 {
        return Err(ConversationWorldStateRepositoryError::Corrupt(
            "active World State full snapshot 的 sequence 不是 0。".to_string(),
        ));
    }
    let mut reducer = WorldStateReducer::new(initial.clone()).map_err(|error| {
        ConversationWorldStateRepositoryError::Corrupt(format!(
            "active World State full snapshot 无效：{error}"
        ))
    })?;
    for entry in entries.iter().skip(1) {
        let WorldStateRecord::Diff(diff) = &entry.record else {
            return Err(ConversationWorldStateRepositoryError::Corrupt(
                "一个 World State epoch 只能包含一个 initial full snapshot。".to_string(),
            ));
        };
        reducer.apply(diff).map_err(|error| {
            ConversationWorldStateRepositoryError::Corrupt(format!(
                "active World State diff 链无法精确折叠：{error}"
            ))
        })?;
    }
    Ok(Some(reducer.into_snapshot()))
}

/// Starts a new exact epoch in its own transaction.
pub fn rebase_active_epoch(
    connection: &mut Connection,
    request: &ConversationWorldStateRebaseRequest<'_>,
) -> Result<ConversationWorldStateRebaseOutcome, ConversationWorldStateRepositoryError> {
    let transaction = connection.transaction()?;
    let outcome = rebase_active_epoch_in_connection(&transaction, request)?;
    transaction.commit()?;
    Ok(outcome)
}

/// Starts a new exact epoch inside the caller's summary-commit transaction.
///
/// Only records effective at or before the summary boundary are folded into the new full snapshot.
/// Later diffs are rewritten onto the new epoch and keep their original message anchors, so
/// compaction cannot make a future state visible earlier than it originally became effective.
///
/// A retry with the same new epoch, summary and boundary is idempotent. A changed source head is
/// stale and rejected, so concurrent state changes can never be silently omitted by compaction.
pub(crate) fn rebase_active_epoch_in_connection(
    connection: &Connection,
    request: &ConversationWorldStateRebaseRequest<'_>,
) -> Result<ConversationWorldStateRebaseOutcome, ConversationWorldStateRepositoryError> {
    validate_rebase_request(request)?;
    let plan = build_rebase_plan(connection, request)?;
    let active_epoch = get_active_epoch(connection, request.conversation_id)?.ok_or_else(|| {
        ConversationWorldStateRepositoryError::Invalid(
            "没有可供 rebase 的 active World State epoch。".to_string(),
        )
    })?;

    if active_epoch.epoch_id == request.new_epoch_id {
        if active_epoch.base_summary_id.as_deref() != Some(request.base_summary_id)
            || active_epoch.generation != plan.source_epoch_generation.saturating_add(1)
        {
            return Err(ConversationWorldStateRepositoryError::Conflict(
                "目标 World State epoch 已存在，但 generation 或压缩摘要边界不一致。".to_string(),
            ));
        }
        let stored = list_active_journal_entries(connection, request.conversation_id)?;
        if stored.len() != plan.records.len()
            || stored.iter().zip(&plan.records).any(|(stored, expected)| {
                stored.epoch_generation != active_epoch.generation
                    || stored.base_summary_id.as_deref() != Some(request.base_summary_id)
                    || stored.effective_before_message_id != expected.effective_before_message_id
                    || stored.record != expected.record
            })
        {
            return Err(ConversationWorldStateRepositoryError::Conflict(
                "目标 World State epoch 与确定性的边界 rebase 结果不一致。".to_string(),
            ));
        }
        let Some(first) = plan.records.first() else {
            return Err(ConversationWorldStateRepositoryError::Corrupt(
                "目标 World State epoch 已存在但没有 full snapshot。".to_string(),
            ));
        };
        let WorldStateRecord::Full(snapshot) = &first.record else {
            return Err(ConversationWorldStateRepositoryError::Corrupt(
                "目标 World State epoch 未以 full snapshot 开始。".to_string(),
            ));
        };
        return Ok(ConversationWorldStateRebaseOutcome {
            append_outcome: ConversationWorldStateAppendOutcome::Idempotent,
            full_snapshot: snapshot.clone(),
            epoch_generation: active_epoch.generation,
        });
    }

    if active_epoch.epoch_id != request.expected_source_epoch_id {
        return Err(ConversationWorldStateRepositoryError::Conflict(format!(
            "active World State epoch 已从 `{}` 变为 `{}`。",
            request.expected_source_epoch_id, active_epoch.epoch_id
        )));
    }
    let epoch_generation = plan.source_epoch_generation.checked_add(1).ok_or_else(|| {
        ConversationWorldStateRepositoryError::Invalid(
            "World State epoch generation 已溢出。".to_string(),
        )
    })?;
    let mut append_outcome = None;
    for planned in &plan.records {
        let outcome = append_record_in_connection(
            connection,
            &ConversationWorldStateRecordWrite {
                conversation_id: request.conversation_id,
                epoch_generation,
                base_summary_id: Some(request.base_summary_id),
                effective_before_message_id: planned.effective_before_message_id.as_deref(),
                record: &planned.record,
                created_at: planned.created_at,
            },
        )?;
        if append_outcome.replace(outcome).is_some()
            && outcome != ConversationWorldStateAppendOutcome::Inserted
        {
            return Err(ConversationWorldStateRepositoryError::Conflict(
                "新 World State epoch 的迁移 diff 出现意外幂等写入。".to_string(),
            ));
        }
    }
    let Some(WorldStateRecord::Full(full_snapshot)) =
        plan.records.first().map(|planned| &planned.record)
    else {
        return Err(ConversationWorldStateRepositoryError::Corrupt(
            "边界 rebase 计划没有 initial full snapshot。".to_string(),
        ));
    };
    Ok(ConversationWorldStateRebaseOutcome {
        append_outcome: append_outcome.unwrap_or(ConversationWorldStateAppendOutcome::Inserted),
        full_snapshot: full_snapshot.clone(),
        epoch_generation,
    })
}

fn build_rebase_plan(
    connection: &Connection,
    request: &ConversationWorldStateRebaseRequest<'_>,
) -> Result<ConversationWorldStateRebasePlan, ConversationWorldStateRepositoryError> {
    let source_epoch = get_epoch(
        connection,
        request.conversation_id,
        request.expected_source_epoch_id,
    )?
    .ok_or_else(|| {
        ConversationWorldStateRepositoryError::Conflict(format!(
            "source World State epoch `{}` 已不存在。",
            request.expected_source_epoch_id
        ))
    })?;
    let cutoff_position = message_position(
        connection,
        request.conversation_id,
        request.covered_through_message_id,
        "compaction cutoff",
    )?;
    let source_records = list_records_for_epoch(
        connection,
        request.conversation_id,
        request.expected_source_epoch_id,
    )?;
    let Some(first) = source_records.first() else {
        return Err(ConversationWorldStateRepositoryError::Corrupt(
            "source World State epoch 没有 initial full snapshot。".to_string(),
        ));
    };
    if first.sequence != 0
        || first.record_kind != ConversationWorldStateRecordKind::Full
        || first.effective_before_message_id.is_some()
    {
        return Err(ConversationWorldStateRepositoryError::Corrupt(
            "source World State epoch 必须从无消息 anchor 的 sequence 0 full snapshot 开始。"
                .to_string(),
        ));
    }
    let WorldStateRecord::Full(initial) = first.decode_record()? else {
        return Err(ConversationWorldStateRepositoryError::Corrupt(
            "source World State epoch 的首条记录不是 full snapshot。".to_string(),
        ));
    };

    let mut source_reducer = WorldStateReducer::new(initial.clone()).map_err(|error| {
        ConversationWorldStateRepositoryError::Corrupt(format!(
            "source World State full snapshot 无效：{error}"
        ))
    })?;
    let mut boundary_snapshot = initial;
    let mut suffix = Vec::new();
    let mut crossed_boundary = false;
    for stored in source_records.iter().skip(1) {
        let WorldStateRecord::Diff(diff) = stored.decode_record()? else {
            return Err(ConversationWorldStateRepositoryError::Corrupt(
                "source World State epoch 在 initial full 后包含了非 diff 记录。".to_string(),
            ));
        };
        let anchor = stored
            .effective_before_message_id
            .as_deref()
            .ok_or_else(|| {
                ConversationWorldStateRepositoryError::Corrupt(
                    "source World State diff 缺少 effective-before message anchor。".to_string(),
                )
            })?;
        let anchor_position = message_position(
            connection,
            request.conversation_id,
            anchor,
            "World State diff anchor",
        )?;
        let effective_at_boundary = anchor_position <= cutoff_position;
        if effective_at_boundary && crossed_boundary {
            return Err(ConversationWorldStateRepositoryError::Corrupt(
                "World State diff anchor 顺序跨越 compaction cutoff 后又回到已覆盖历史。"
                    .to_string(),
            ));
        }
        source_reducer.apply(&diff).map_err(|error| {
            ConversationWorldStateRepositoryError::Corrupt(format!(
                "source World State diff 链无法精确折叠：{error}"
            ))
        })?;
        if effective_at_boundary {
            boundary_snapshot = source_reducer.snapshot().clone();
        } else {
            crossed_boundary = true;
            suffix.push((stored, diff));
        }
    }
    let source_head = source_reducer.into_snapshot();
    if source_head.revision != request.expected_source_revision {
        return Err(ConversationWorldStateRepositoryError::Conflict(
            "active World State revision 已变化，不能提交旧的 compaction rebase。".to_string(),
        ));
    }

    let full_snapshot = boundary_snapshot
        .rebase(request.new_epoch_id)
        .map_err(|error| {
            ConversationWorldStateRepositoryError::Invalid(format!(
                "无法构造边界 rebased World State full snapshot：{error}"
            ))
        })?;
    let mut target_reducer = WorldStateReducer::new(full_snapshot.clone()).map_err(|error| {
        ConversationWorldStateRepositoryError::Invalid(format!(
            "边界 rebased World State full snapshot 无效：{error}"
        ))
    })?;
    let mut records = vec![RebasedWorldStateRecord {
        record: WorldStateRecord::Full(full_snapshot),
        effective_before_message_id: None,
        created_at: request.created_at,
    }];
    for (stored, source_diff) in suffix {
        let sequence = target_reducer
            .snapshot()
            .sequence
            .checked_add(1)
            .ok_or_else(|| {
                ConversationWorldStateRepositoryError::Invalid(
                    "迁移 World State diff sequence 已溢出。".to_string(),
                )
            })?;
        let source_result_revision = source_diff.result_revision.clone();
        let migrated = WorldStateDiff::from_operations(
            target_reducer.snapshot(),
            sequence,
            source_diff.operations,
        )
        .map_err(|error| {
            ConversationWorldStateRepositoryError::Corrupt(format!(
                "无法将 cutoff 后 World State diff 重写到新 epoch：{error}"
            ))
        })?;
        if migrated.result_revision != source_result_revision {
            return Err(ConversationWorldStateRepositoryError::Corrupt(
                "迁移 World State diff 改变了其确定性结果 revision。".to_string(),
            ));
        }
        target_reducer.apply(&migrated).map_err(|error| {
            ConversationWorldStateRepositoryError::Corrupt(format!(
                "迁移后的 World State diff 链无效：{error}"
            ))
        })?;
        records.push(RebasedWorldStateRecord {
            record: WorldStateRecord::Diff(migrated),
            effective_before_message_id: stored.effective_before_message_id.clone(),
            created_at: stored.created_at,
        });
    }
    if target_reducer.snapshot().revision != source_head.revision {
        return Err(ConversationWorldStateRepositoryError::Corrupt(
            "边界 rebase 后的 World State head 与 source head 不一致。".to_string(),
        ));
    }
    Ok(ConversationWorldStateRebasePlan {
        source_epoch_generation: source_epoch.generation,
        records,
    })
}

fn message_position(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
    label: &str,
) -> Result<i64, ConversationWorldStateRepositoryError> {
    connection
        .query_row(
            "SELECT position
             FROM messages
             WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .ok_or_else(|| {
            ConversationWorldStateRepositoryError::Invalid(format!(
                "{label} `{message_id}` 不属于会话 `{conversation_id}`。"
            ))
        })
}

/// Deletes one epoch and every record it owns.
pub fn delete_epoch(
    connection: &Connection,
    conversation_id: &str,
    epoch_id: &str,
) -> Result<bool, ConversationWorldStateRepositoryError> {
    Ok(connection.execute(
        "DELETE FROM conversation_world_state_epochs
         WHERE conversation_id = ?1 AND epoch_id = ?2",
        params![conversation_id, epoch_id],
    )? > 0)
}

/// Rewinds the journal to an exact record and removes all causally later epochs/records.
pub fn rewind_after(
    connection: &mut Connection,
    conversation_id: &str,
    epoch_id: &str,
    sequence: u64,
) -> Result<(), ConversationWorldStateRepositoryError> {
    let transaction = connection.transaction()?;
    rewind_after_in_connection(&transaction, conversation_id, epoch_id, sequence)?;
    transaction.commit()?;
    Ok(())
}

pub(crate) fn rewind_after_in_connection(
    connection: &Connection,
    conversation_id: &str,
    epoch_id: &str,
    sequence: u64,
) -> Result<(), ConversationWorldStateRepositoryError> {
    let boundary = connection
        .query_row(
            "SELECT record.journal_position, epoch.generation
             FROM conversation_world_state_records AS record
             INNER JOIN conversation_world_state_epochs AS epoch
               ON epoch.conversation_id = record.conversation_id
              AND epoch.epoch_id = record.epoch_id
             WHERE record.conversation_id = ?1
               AND record.epoch_id = ?2
               AND record.sequence = ?3",
            params![
                conversation_id,
                epoch_id,
                sqlite_u64(sequence, "record sequence")?
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
        .ok_or_else(|| {
            ConversationWorldStateRepositoryError::Invalid(
                "World State rewind 边界不存在。".to_string(),
            )
        })?;

    connection.execute(
        "DELETE FROM conversation_world_state_epochs
         WHERE conversation_id = ?1 AND generation > ?2",
        params![conversation_id, boundary.1],
    )?;
    connection.execute(
        "DELETE FROM conversation_world_state_records
         WHERE conversation_id = ?1
           AND epoch_id = ?2
           AND journal_position > ?3",
        params![conversation_id, epoch_id, boundary.0],
    )?;
    Ok(())
}

/// Rewinds records whose effective point is at or after the earliest deleted message.
///
/// This must run before the message rows are removed so their original positions remain
/// available. It is safe inside the same transaction as message deletion. The anchor foreign key
/// is still retained as a final integrity backstop.
pub(crate) fn rewind_for_message_deletion(
    connection: &Connection,
    conversation_id: &str,
    message_ids: &[String],
) -> Result<(), ConversationWorldStateRepositoryError> {
    if message_ids.is_empty() {
        return Ok(());
    }

    let placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let minimum_deleted_position = connection.query_row(
        &format!(
            "SELECT MIN(position)
                 FROM messages
                 WHERE conversation_id = ? AND id IN ({placeholders})"
        ),
        rusqlite::params_from_iter(
            std::iter::once(conversation_id).chain(message_ids.iter().map(String::as_str)),
        ),
        |row| row.get::<_, Option<i64>>(0),
    )?;
    let Some(minimum_deleted_position) = minimum_deleted_position else {
        return Ok(());
    };

    let first_affected_journal_position = connection.query_row(
        "SELECT MIN(record.journal_position)
             FROM conversation_world_state_records AS record
             INNER JOIN messages AS anchor
               ON anchor.id = record.effective_before_message_id
              AND anchor.conversation_id = record.conversation_id
             WHERE record.conversation_id = ?1
               AND anchor.position >= ?2",
        params![conversation_id, minimum_deleted_position],
        |row| row.get::<_, Option<i64>>(0),
    )?;
    let Some(first_affected_journal_position) = first_affected_journal_position else {
        return Ok(());
    };

    connection.execute(
        "DELETE FROM conversation_world_state_records
         WHERE conversation_id = ?1 AND journal_position >= ?2",
        params![conversation_id, first_affected_journal_position],
    )?;
    delete_empty_epochs(connection, conversation_id)?;
    Ok(())
}

#[derive(Debug)]
struct StoredEpoch {
    epoch_id: String,
    generation: u64,
    base_summary_id: Option<String>,
}

fn get_active_epoch(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<StoredEpoch>, ConversationWorldStateRepositoryError> {
    connection
        .query_row(
            "SELECT epoch_id, generation, base_summary_id
             FROM conversation_world_state_epochs
             WHERE conversation_id = ?1
             ORDER BY generation DESC
             LIMIT 1",
            [conversation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?
        .map(|(epoch_id, generation, base_summary_id)| {
            Ok(StoredEpoch {
                epoch_id,
                generation: storage_u64(generation, "epoch generation")?,
                base_summary_id,
            })
        })
        .transpose()
}

fn get_epoch(
    connection: &Connection,
    conversation_id: &str,
    epoch_id: &str,
) -> Result<Option<StoredEpoch>, ConversationWorldStateRepositoryError> {
    connection
        .query_row(
            "SELECT epoch_id, generation, base_summary_id
             FROM conversation_world_state_epochs
             WHERE conversation_id = ?1 AND epoch_id = ?2",
            params![conversation_id, epoch_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?
        .map(|(epoch_id, generation, base_summary_id)| {
            Ok(StoredEpoch {
                epoch_id,
                generation: storage_u64(generation, "epoch generation")?,
                base_summary_id,
            })
        })
        .transpose()
}

fn get_record(
    connection: &Connection,
    conversation_id: &str,
    epoch_id: &str,
    sequence: u64,
) -> Result<Option<StoredConversationWorldStateRecord>, ConversationWorldStateRepositoryError> {
    let row = connection
        .query_row(
            "SELECT
                record.conversation_id,
                record.schema_version,
                record.epoch_id,
                epoch.generation,
                epoch.base_summary_id,
                record.sequence,
                record.record_kind,
                record.base_revision,
                record.result_revision,
                record.effective_before_message_id,
                record.record_json,
                record.created_at
             FROM conversation_world_state_records AS record
             INNER JOIN conversation_world_state_epochs AS epoch
               ON epoch.conversation_id = record.conversation_id
              AND epoch.epoch_id = record.epoch_id
             WHERE record.conversation_id = ?1
               AND record.epoch_id = ?2
               AND record.sequence = ?3",
            params![
                conversation_id,
                epoch_id,
                sqlite_u64(sequence, "record sequence")?
            ],
            stored_record_from_row,
        )
        .optional()?;
    row.map(validate_stored_record).transpose()
}

fn validate_write(
    record: &IndexedWorldStateRecordWrite<'_>,
) -> Result<(), ConversationWorldStateRepositoryError> {
    for (label, value, max_bytes) in [
        ("conversation id", record.conversation_id, usize::MAX),
        ("epoch id", record.epoch_id, 256),
        ("result revision", record.result_revision, 256),
    ] {
        if value.trim().is_empty() || value.len() > max_bytes {
            return Err(ConversationWorldStateRepositoryError::Invalid(format!(
                "{label} 为空或超过长度限制。"
            )));
        }
    }
    if record.schema_version == 0 || record.epoch_generation == 0 || record.created_at < 0 {
        return Err(ConversationWorldStateRepositoryError::Invalid(
            "schema version、epoch generation 和创建时间无效。".to_string(),
        ));
    }
    match record.record_kind {
        ConversationWorldStateRecordKind::Full if record.base_revision.is_some() => {
            return Err(ConversationWorldStateRepositoryError::Invalid(
                "full snapshot 不能声明 base revision。".to_string(),
            ))
        }
        ConversationWorldStateRecordKind::Diff
            if record
                .base_revision
                .is_none_or(|revision| revision.trim().is_empty() || revision.len() > 256) =>
        {
            return Err(ConversationWorldStateRepositoryError::Invalid(
                "diff 必须声明有效的 base revision。".to_string(),
            ))
        }
        _ => {}
    }
    if record.record_kind == ConversationWorldStateRecordKind::Diff
        && record.effective_before_message_id.is_none()
    {
        return Err(ConversationWorldStateRepositoryError::Invalid(
            "conversation World State diff 必须具有 effective-before message anchor。".to_string(),
        ));
    }
    Ok(())
}

fn validate_rebase_request(
    request: &ConversationWorldStateRebaseRequest<'_>,
) -> Result<(), ConversationWorldStateRepositoryError> {
    for (label, value) in [
        ("conversation id", request.conversation_id),
        ("source epoch id", request.expected_source_epoch_id),
        ("source revision", request.expected_source_revision),
        (
            "covered-through message id",
            request.covered_through_message_id,
        ),
        ("new epoch id", request.new_epoch_id),
        ("base summary id", request.base_summary_id),
    ] {
        if value.trim().is_empty() {
            return Err(ConversationWorldStateRepositoryError::Invalid(format!(
                "{label} 不能为空。"
            )));
        }
    }
    if request.expected_source_epoch_id == request.new_epoch_id || request.created_at < 0 {
        return Err(ConversationWorldStateRepositoryError::Invalid(
            "rebase 必须使用新的 epoch id 和有效创建时间。".to_string(),
        ));
    }
    Ok(())
}

fn validate_new_epoch(
    record: &IndexedWorldStateRecordWrite<'_>,
    active_epoch: Option<&StoredEpoch>,
) -> Result<(), ConversationWorldStateRepositoryError> {
    if record.record_kind != ConversationWorldStateRecordKind::Full || record.sequence != 0 {
        return Err(ConversationWorldStateRepositoryError::Invalid(
            "新的 World State epoch 必须从 sequence 0 的 full snapshot 开始。".to_string(),
        ));
    }
    let expected_generation = active_epoch
        .map(|epoch| epoch.generation.saturating_add(1))
        .unwrap_or(1);
    if record.epoch_generation != expected_generation {
        return Err(ConversationWorldStateRepositoryError::Conflict(format!(
            "新的 World State epoch generation 应为 {expected_generation}。"
        )));
    }
    Ok(())
}

fn validate_next_record(
    connection: &Connection,
    record: &IndexedWorldStateRecordWrite<'_>,
) -> Result<(), ConversationWorldStateRepositoryError> {
    let head = get_active_head(connection, record.conversation_id)?.ok_or_else(|| {
        ConversationWorldStateRepositoryError::Corrupt(
            "World State epoch 已存在但没有 initial full snapshot。".to_string(),
        )
    })?;
    if head.epoch_id != record.epoch_id {
        return Err(ConversationWorldStateRepositoryError::Conflict(
            "只能向 active World State epoch 追加记录。".to_string(),
        ));
    }
    let expected_sequence = head.sequence.checked_add(1).ok_or_else(|| {
        ConversationWorldStateRepositoryError::Invalid("World State sequence 已溢出。".to_string())
    })?;
    if record.sequence != expected_sequence
        || record.record_kind != ConversationWorldStateRecordKind::Diff
        || record.base_revision != Some(head.result_revision.as_str())
    {
        return Err(ConversationWorldStateRepositoryError::Conflict(format!(
            "diff 必须以 sequence {expected_sequence} 和当前 head revision 为基础。"
        )));
    }
    Ok(())
}

fn validate_exact_next_diff(
    connection: &Connection,
    record: &IndexedWorldStateRecordWrite<'_>,
) -> Result<(), ConversationWorldStateRepositoryError> {
    let domain_record =
        serde_json::from_str::<WorldStateRecord>(&record.record_json).map_err(|error| {
            ConversationWorldStateRepositoryError::Invalid(format!(
                "待追加 World State record 无法解码：{error}"
            ))
        })?;
    let WorldStateRecord::Diff(diff) = domain_record else {
        return Err(ConversationWorldStateRepositoryError::Invalid(
            "现有 World State epoch 只能追加 diff。".to_string(),
        ));
    };
    let current = fold_active_snapshot(connection, record.conversation_id)?.ok_or_else(|| {
        ConversationWorldStateRepositoryError::Corrupt(
            "active World State epoch 没有可折叠状态。".to_string(),
        )
    })?;
    let mut reducer = WorldStateReducer::new(current).map_err(|error| {
        ConversationWorldStateRepositoryError::Corrupt(format!(
            "active World State snapshot 无效：{error}"
        ))
    })?;
    reducer.apply(&diff).map_err(|error| {
        ConversationWorldStateRepositoryError::Invalid(format!(
            "待追加 World State diff 无法精确应用：{error}"
        ))
    })
}

fn semantically_matches(
    write: &IndexedWorldStateRecordWrite<'_>,
    stored: &StoredConversationWorldStateRecord,
) -> bool {
    stored.conversation_id == write.conversation_id
        && stored.schema_version == write.schema_version
        && stored.epoch_id == write.epoch_id
        && stored.epoch_generation == write.epoch_generation
        && stored.base_summary_id.as_deref() == write.base_summary_id
        && stored.sequence == write.sequence
        && stored.record_kind == write.record_kind
        && stored.base_revision.as_deref() == write.base_revision
        && stored.result_revision == write.result_revision
        && stored.effective_before_message_id.as_deref() == write.effective_before_message_id
        && stored.record_json == write.record_json
}

fn stored_record_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StoredConversationWorldStateRecord> {
    let schema_version = row.get::<_, i64>(1)?;
    let epoch_generation = row.get::<_, i64>(3)?;
    let sequence = row.get::<_, i64>(5)?;
    let record_kind_raw = row.get::<_, String>(6)?;
    let record_kind =
        ConversationWorldStateRecordKind::from_str(&record_kind_raw).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unknown world state record kind: {record_kind_raw}"),
                )),
            )
        })?;
    Ok(StoredConversationWorldStateRecord {
        conversation_id: row.get(0)?,
        schema_version: u32::try_from(schema_version).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        epoch_id: row.get(2)?,
        epoch_generation: u64::try_from(epoch_generation).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        base_summary_id: row.get(4)?,
        sequence: u64::try_from(sequence).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        record_kind,
        base_revision: row.get(7)?,
        result_revision: row.get(8)?,
        effective_before_message_id: row.get(9)?,
        record_json: row.get(10)?,
        created_at: row.get(11)?,
    })
}

fn validate_stored_record(
    record: StoredConversationWorldStateRecord,
) -> Result<StoredConversationWorldStateRecord, ConversationWorldStateRepositoryError> {
    let canonical = canonicalize_json(&record.record_json).map_err(|error| {
        ConversationWorldStateRepositoryError::Corrupt(format!(
            "epoch `{}` sequence {} 的 JSON 无法解码：{error}",
            record.epoch_id, record.sequence
        ))
    })?;
    if canonical != record.record_json {
        return Err(ConversationWorldStateRepositoryError::Corrupt(format!(
            "epoch `{}` sequence {} 的 record JSON 不是 canonical encoding。",
            record.epoch_id, record.sequence
        )));
    }
    record.decode_record()?;
    Ok(record)
}

fn delete_empty_epochs(
    connection: &Connection,
    conversation_id: &str,
) -> Result<(), ConversationWorldStateRepositoryError> {
    connection.execute(
        "DELETE FROM conversation_world_state_epochs
         WHERE conversation_id = ?1
           AND NOT EXISTS (
               SELECT 1
               FROM conversation_world_state_records AS record
               WHERE record.conversation_id = conversation_world_state_epochs.conversation_id
                 AND record.epoch_id = conversation_world_state_epochs.epoch_id
           )",
        [conversation_id],
    )?;
    Ok(())
}

fn sqlite_u64(value: u64, label: &str) -> Result<i64, ConversationWorldStateRepositoryError> {
    i64::try_from(value).map_err(|_| {
        ConversationWorldStateRepositoryError::Invalid(format!(
            "{label} 超过 SQLite INTEGER 范围。"
        ))
    })
}

fn storage_u64(value: i64, label: &str) -> Result<u64, ConversationWorldStateRepositoryError> {
    u64::try_from(value)
        .map_err(|_| ConversationWorldStateRepositoryError::Corrupt(format!("{label} 为负数。")))
}

fn canonicalize_json(raw: &str) -> Result<String, ConversationWorldStateRepositoryError> {
    let value = serde_json::from_str::<Value>(raw).map_err(|error| {
        ConversationWorldStateRepositoryError::Invalid(format!("record JSON 无效：{error}"))
    })?;
    let mut output = String::new();
    write_canonical_json(&value, &mut output)?;
    Ok(output)
}

fn write_canonical_json(
    value: &Value,
    output: &mut String,
) -> Result<(), ConversationWorldStateRepositoryError> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&value.to_string()),
        Value::String(value) => {
            output.push_str(&serde_json::to_string(value).map_err(|error| {
                ConversationWorldStateRepositoryError::Invalid(format!(
                    "record JSON 字符串无法序列化：{error}"
                ))
            })?)
        }
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(key).map_err(|error| {
                    ConversationWorldStateRepositoryError::Invalid(format!(
                        "record JSON key 无法序列化：{error}"
                    ))
                })?);
                output.push(':');
                write_canonical_json(&values[key], output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{chat_repository, migrations};
    use crate::{
        WorldStateDiff, WorldStateLifetime, WorldStateSectionEnvelope, WorldStateSectionId,
        WorldStateSnapshot,
    };
    use rusqlite::params;
    use serde_json::json;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        connection
    }

    fn insert_conversation(connection: &Connection, conversation_id: &str) {
        connection
            .execute(
                "INSERT INTO conversations (id, title, created_at, updated_at)
                 VALUES (?1, ?1, 1, 1)",
                [conversation_id],
            )
            .unwrap();
    }

    fn insert_message(
        connection: &Connection,
        conversation_id: &str,
        message_id: &str,
        position: i64,
    ) {
        connection
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, role, content, created_at, position
                 ) VALUES (?1, ?2, 'user', 'message', ?3, ?3)",
                params![message_id, conversation_id, position],
            )
            .unwrap();
    }

    fn insert_summary(
        connection: &Connection,
        conversation_id: &str,
        message_id: &str,
        summary_id: &str,
    ) {
        connection
            .execute(
                "INSERT INTO context_compaction_summaries (
                    id,
                    conversation_id,
                    schema_version,
                    source_revision,
                    previous_summary_id,
                    covered_through_kind,
                    covered_through_message_id,
                    covered_through_trace_sequence,
                    content,
                    continuity_schema_version,
                    continuity_json,
                    generation_kind,
                    generation_model,
                    source_input_tokens,
                    summary_input_tokens,
                    continuity_input_tokens,
                    replacement_input_tokens,
                    created_at
                 ) VALUES (
                    ?1, ?2, ?3, 'source-revision', NULL, 'message', ?4, NULL,
                    'historical summary', ?5, '{}', 'test', NULL, 10, 2, 2, 4, 20
                 )",
                params![
                    summary_id,
                    conversation_id,
                    crate::CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
                    message_id,
                    crate::CONTEXT_CONTINUITY_SCHEMA_VERSION,
                ],
            )
            .unwrap();
    }

    fn snapshot(epoch_id: &str, sequence: u64, value: &str) -> WorldStateSnapshot {
        WorldStateSnapshot::new(
            epoch_id,
            sequence,
            vec![WorldStateSectionEnvelope::model_visible(
                WorldStateSectionId::EffectivePermissions,
                WorldStateLifetime::Conversation,
                json!({ "value": value, "hostOnlyDetail": "not projected" }),
                json!({ "value": value }),
            )
            .unwrap()],
        )
        .unwrap()
    }

    fn append(
        connection: &mut Connection,
        conversation_id: &str,
        generation: u64,
        anchor: Option<&str>,
        record: &WorldStateRecord,
        created_at: i64,
    ) -> Result<ConversationWorldStateAppendOutcome, ConversationWorldStateRepositoryError> {
        append_record(
            connection,
            &ConversationWorldStateRecordWrite {
                conversation_id,
                epoch_generation: generation,
                base_summary_id: None,
                effective_before_message_id: anchor,
                record,
                created_at,
            },
        )
    }

    #[test]
    fn append_is_idempotent_but_rejects_conflicting_duplicate_and_sequence_gap() {
        let mut connection = connection();
        insert_conversation(&connection, "conversation");
        insert_message(&connection, "conversation", "message-0", 0);
        insert_message(&connection, "conversation", "message-1", 1);
        let full = snapshot("epoch-1", 0, "ask");
        let target = snapshot("epoch-1", 1, "allow");
        let diff = WorldStateDiff::between(&full, &target).unwrap();
        let full_record = WorldStateRecord::Full(full);
        let diff_record = WorldStateRecord::Diff(diff);

        assert_eq!(
            append(
                &mut connection,
                "conversation",
                1,
                Some("message-0"),
                &full_record,
                10,
            )
            .unwrap(),
            ConversationWorldStateAppendOutcome::Inserted
        );
        // Retries may observe a different wall-clock time; immutable semantic identity still makes
        // this the same append.
        assert_eq!(
            append(
                &mut connection,
                "conversation",
                1,
                Some("message-0"),
                &full_record,
                99,
            )
            .unwrap(),
            ConversationWorldStateAppendOutcome::Idempotent
        );
        assert!(matches!(
            append(
                &mut connection,
                "conversation",
                1,
                Some("message-1"),
                &full_record,
                10,
            ),
            Err(ConversationWorldStateRepositoryError::Conflict(_))
        ));

        let skipped_target = snapshot("epoch-1", 2, "skip");
        let mut skipped_diff = match diff_record.clone() {
            WorldStateRecord::Diff(diff) => diff,
            WorldStateRecord::Full(_) => unreachable!(),
        };
        skipped_diff.sequence = 2;
        skipped_diff.result_revision = skipped_target.revision;
        assert!(matches!(
            append(
                &mut connection,
                "conversation",
                1,
                Some("message-1"),
                &WorldStateRecord::Diff(skipped_diff),
                11,
            ),
            Err(ConversationWorldStateRepositoryError::Conflict(_))
        ));

        assert_eq!(
            append(
                &mut connection,
                "conversation",
                1,
                Some("message-1"),
                &diff_record,
                11,
            )
            .unwrap(),
            ConversationWorldStateAppendOutcome::Inserted
        );
        let records = list_active_journal_entries(&connection, "conversation").unwrap();
        assert_eq!(
            records
                .iter()
                .map(|entry| entry.record.sequence())
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(
            get_active_head_entry(&connection, "conversation")
                .unwrap()
                .unwrap()
                .record,
            diff_record
        );
    }

    #[test]
    fn latest_epoch_is_active_and_exact_rewind_restores_the_previous_head() {
        let mut connection = connection();
        insert_conversation(&connection, "conversation");
        insert_message(&connection, "conversation", "message-1", 1);
        let full_one = snapshot("epoch-1", 0, "ask");
        let state_one = snapshot("epoch-1", 1, "allow");
        let diff_one = WorldStateDiff::between(&full_one, &state_one).unwrap();
        append(
            &mut connection,
            "conversation",
            1,
            None,
            &WorldStateRecord::Full(full_one),
            10,
        )
        .unwrap();
        append(
            &mut connection,
            "conversation",
            1,
            Some("message-1"),
            &WorldStateRecord::Diff(diff_one),
            11,
        )
        .unwrap();

        let full_two = WorldStateRecord::Full(state_one.rebase("epoch-2").unwrap());
        append(&mut connection, "conversation", 2, None, &full_two, 12).unwrap();
        let active = list_active_journal_entries(&connection, "conversation").unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].record, full_two);
        assert_eq!(active[0].epoch_generation, 2);

        rewind_after(&mut connection, "conversation", "epoch-1", 0).unwrap();
        let active = list_active_journal_entries(&connection, "conversation").unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].record.epoch_id(), "epoch-1");
        assert_eq!(active[0].record.sequence(), 0);
    }

    #[test]
    fn deleting_a_message_rewinds_every_later_anchored_state_record() {
        let mut connection = connection();
        insert_conversation(&connection, "conversation");
        for position in 0..4 {
            insert_message(
                &connection,
                "conversation",
                &format!("message-{position}"),
                position,
            );
        }
        let full = snapshot("epoch", 0, "zero");
        let one = snapshot("epoch", 1, "one");
        let diff_one = WorldStateDiff::between(&full, &one).unwrap();
        let two = snapshot("epoch", 2, "two");
        let diff_two = WorldStateDiff::between(&one, &two).unwrap();
        append(
            &mut connection,
            "conversation",
            1,
            Some("message-0"),
            &WorldStateRecord::Full(full),
            10,
        )
        .unwrap();
        append(
            &mut connection,
            "conversation",
            1,
            Some("message-2"),
            &WorldStateRecord::Diff(diff_one),
            11,
        )
        .unwrap();
        append(
            &mut connection,
            "conversation",
            1,
            Some("message-3"),
            &WorldStateRecord::Diff(diff_two),
            12,
        )
        .unwrap();

        // The deleted message has no direct World State anchor. Position-aware rewind still
        // removes records that would otherwise describe the abandoned future branch.
        chat_repository::delete_messages(
            &mut connection,
            "conversation",
            &["message-1".to_string()],
        )
        .unwrap();
        let active = list_active_journal_entries(&connection, "conversation").unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].record.sequence(), 0);
    }

    #[test]
    fn compaction_rebase_is_exact_atomic_idempotent_and_summary_owned() {
        let mut connection = connection();
        insert_conversation(&connection, "conversation");
        insert_message(&connection, "conversation", "message-0", 0);
        insert_message(&connection, "conversation", "message-1", 1);
        insert_message(&connection, "conversation", "message-2", 2);
        let full = snapshot("epoch-1", 0, "ask");
        let at_cutoff = snapshot("epoch-1", 1, "allow");
        let cutoff_diff = WorldStateDiff::between(&full, &at_cutoff).unwrap();
        let current = snapshot("epoch-1", 2, "deny");
        let future_diff = WorldStateDiff::between(&at_cutoff, &current).unwrap();
        append(
            &mut connection,
            "conversation",
            1,
            None,
            &WorldStateRecord::Full(full),
            10,
        )
        .unwrap();
        append(
            &mut connection,
            "conversation",
            1,
            Some("message-1"),
            &WorldStateRecord::Diff(cutoff_diff),
            11,
        )
        .unwrap();
        append(
            &mut connection,
            "conversation",
            1,
            Some("message-2"),
            &WorldStateRecord::Diff(future_diff),
            12,
        )
        .unwrap();
        assert_eq!(
            fold_active_snapshot(&connection, "conversation").unwrap(),
            Some(current.clone())
        );

        insert_summary(&connection, "conversation", "message-1", "summary-1");
        let request = ConversationWorldStateRebaseRequest {
            conversation_id: "conversation",
            expected_source_epoch_id: "epoch-1",
            expected_source_revision: &current.revision,
            covered_through_message_id: "message-1",
            new_epoch_id: "epoch-2",
            base_summary_id: "summary-1",
            created_at: 21,
        };
        let inserted = rebase_active_epoch(&mut connection, &request).unwrap();
        assert_eq!(
            inserted.append_outcome,
            ConversationWorldStateAppendOutcome::Inserted
        );
        assert_eq!(inserted.full_snapshot.sequence, 0);
        assert_eq!(inserted.full_snapshot.revision, at_cutoff.revision);
        assert_eq!(inserted.epoch_generation, 2);

        let retried = rebase_active_epoch(&mut connection, &request).unwrap();
        assert_eq!(
            retried.append_outcome,
            ConversationWorldStateAppendOutcome::Idempotent
        );
        assert_eq!(retried.full_snapshot, inserted.full_snapshot);
        assert_eq!(
            list_records_for_epoch(&connection, "conversation", "epoch-1")
                .unwrap()
                .len(),
            3
        );
        let active = list_active_journal_entries(&connection, "conversation").unwrap();
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].base_summary_id.as_deref(), Some("summary-1"));
        assert_eq!(
            active[1].effective_before_message_id.as_deref(),
            Some("message-2")
        );
        assert_eq!(active[1].record.epoch_id(), "epoch-2");
        assert_eq!(active[1].record.sequence(), 1);
        assert_eq!(
            active[1].record.base_revision(),
            Some(inserted.full_snapshot.revision.as_str())
        );
        assert_eq!(active[1].record.revision(), current.revision);
        insert_summary(&connection, "conversation", "message-1", "summary-2");
        let stale_request = ConversationWorldStateRebaseRequest {
            conversation_id: "conversation",
            expected_source_epoch_id: "epoch-2",
            expected_source_revision: &inserted.full_snapshot.revision,
            covered_through_message_id: "message-1",
            new_epoch_id: "epoch-3",
            base_summary_id: "summary-2",
            created_at: 23,
        };
        assert!(matches!(
            rebase_active_epoch(&mut connection, &stale_request),
            Err(ConversationWorldStateRepositoryError::Conflict(_))
        ));

        // The exact epoch is owned by its compaction summary. If that summary is rewound, SQLite
        // drops the derived epoch and the previous exact epoch becomes active again.
        connection
            .execute(
                "DELETE FROM context_compaction_summaries WHERE id = 'summary-1'",
                [],
            )
            .unwrap();
        let restored = fold_active_snapshot(&connection, "conversation")
            .unwrap()
            .unwrap();
        assert_eq!(restored, current);
        assert_eq!(
            get_active_head_entry(&connection, "conversation")
                .unwrap()
                .unwrap()
                .record
                .epoch_id(),
            "epoch-1"
        );
    }

    #[test]
    fn conversation_delete_cascades_and_legacy_conversations_have_an_empty_journal() {
        let mut connection = connection();
        insert_conversation(&connection, "legacy");
        assert!(list_active_journal_entries(&connection, "legacy")
            .unwrap()
            .is_empty());
        assert!(get_active_head_entry(&connection, "legacy")
            .unwrap()
            .is_none());

        insert_conversation(&connection, "delete");
        let full = WorldStateRecord::Full(snapshot("epoch", 0, "ask"));
        append(&mut connection, "delete", 1, None, &full, 10).unwrap();
        chat_repository::delete_conversation(&connection, "delete").unwrap();

        let counts = connection
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM conversation_world_state_epochs),
                    (SELECT COUNT(*) FROM conversation_world_state_records)",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (0, 0));
    }

    #[test]
    fn anchor_and_epoch_summary_must_belong_to_the_same_conversation() {
        let mut connection = connection();
        insert_conversation(&connection, "conversation-a");
        insert_conversation(&connection, "conversation-b");
        insert_message(&connection, "conversation-b", "foreign-message", 0);
        let full = WorldStateRecord::Full(snapshot("epoch", 0, "ask"));

        assert!(matches!(
            append(
                &mut connection,
                "conversation-a",
                1,
                Some("foreign-message"),
                &full,
                10,
            ),
            Err(ConversationWorldStateRepositoryError::Database(_))
        ));
        assert!(list_active_journal_entries(&connection, "conversation-a")
            .unwrap()
            .is_empty());
    }
}
