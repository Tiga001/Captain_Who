use super::*;

/// Complete recovery facts plus a bounded animation window. Call under a read transaction so
/// every fact belongs to the advertised sequence. No letter bodies or input JSON are selected.
pub fn runtime_summary(
    c: &Connection,
    instance_id: &str,
    after_sequence: Option<u64>,
) -> Result<RuntimeSnapshot, String> {
    let exists: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM workflow_instances WHERE instance_id=?1)",
            [instance_id],
            |row| row.get(0),
        )
        .map_err(db)?;
    if !exists {
        return Err("Organization instance no longer exists".into());
    }
    // Separate indexed MAX lookups: combining structure with a CASE aggregate scans history.
    let (sequence, structure_revision): (u64, u64) = c
        .query_row(
            "SELECT
             (SELECT COALESCE(MAX(sequence),0) FROM workflow_mail_events WHERE instance_id=?1),
             (SELECT COALESCE(MAX(sequence),0) FROM workflow_mail_events
              WHERE instance_id=?1 AND kind='members_changed')",
            [instance_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db)?;
    // Preference events are recovery invalidations, not replayed preference values. Retain the
    // latest event of each field for current nodes even after it leaves the animation window.
    // Probe each bound node/field instead of grouping all historical preference events.
    let mut statement = c
        .prepare(
            "WITH recent AS (
             SELECT sequence FROM workflow_mail_events
             WHERE instance_id=?1 AND sequence>?2 ORDER BY sequence DESC LIMIT 512
         ), fields(kind) AS (
             VALUES ('member_model_changed'),('member_permissions_changed')
         ), preferences AS (
             SELECT (SELECT MAX(event.sequence) FROM workflow_mail_events AS event
                     WHERE event.instance_id=binding.instance_id
                       AND event.target_node_id=binding.node_id AND event.kind=fields.kind
                       AND event.kind IN ('member_model_changed','member_permissions_changed'))
                 AS sequence
             FROM workflow_instance_bindings AS binding CROSS JOIN fields
             WHERE binding.instance_id=?1
         )
         SELECT sequence,input_id,message_id,source_node_id,target_node_id,kind,created_at
         FROM workflow_mail_events WHERE sequence IN (
             SELECT sequence FROM recent UNION SELECT sequence FROM preferences
         ) ORDER BY sequence",
        )
        .map_err(db)?;
    let events = statement
        .query_map(
            params![
                instance_id,
                after_sequence.map_or(0, |s| i64::try_from(s).unwrap_or(i64::MAX))
            ],
            |row| {
                Ok(Event {
                    sequence: row.get(0)?,
                    instance_id: instance_id.into(),
                    input_id: row.get(1)?,
                    message_id: row.get(2)?,
                    source_node_id: row.get(3)?,
                    target_node_id: row.get(4)?,
                    kind: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
        .map_err(db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db)?;
    let mut statement = c.prepare(
        "SELECT mail.node_id,COUNT(*) FROM workflow_instance_bindings AS binding
         CROSS JOIN workflow_mail_messages AS mail
         WHERE binding.instance_id=?1 AND mail.instance_id=binding.instance_id
           AND mail.node_id=binding.node_id AND mail.recipient_conversation_id=binding.conversation_id
           AND mail.mail_status='pending'
         GROUP BY mail.node_id ORDER BY mail.node_id",
    ).map_err(db)?;
    let pending_by_node = statement
        .query_map([instance_id], |row| {
            Ok(PendingNodeMail {
                node_id: row.get(0)?,
                count: row.get(1)?,
            })
        })
        .map_err(db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db)?;
    // Never limit this to recent inputs/events or current bindings: a removed member's final
    // run can settle an older receipt long after its delivery left those windows.
    // The transaction-maintained metadata table avoids joining every historical input/event.
    let mut statement = c
        .prepare(
            "SELECT conversation_id,last_sequence FROM workflow_mail_conversation_changes
         WHERE instance_id=?1 ORDER BY conversation_id",
        )
        .map_err(db)?;
    let conversation_changes = statement
        .query_map([instance_id], |row| {
            Ok(ConversationMailChange {
                conversation_id: row.get(0)?,
                sequence: row.get(1)?,
            })
        })
        .map_err(db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db)?;
    let mut statement = c
        .prepare(
            "SELECT pause.conversation_id FROM workflow_mail_pauses AS pause
         JOIN workflow_instance_bindings AS binding ON binding.conversation_id=pause.conversation_id
         WHERE binding.instance_id=?1 ORDER BY pause.conversation_id",
        )
        .map_err(db)?;
    let paused_conversation_ids = statement
        .query_map([instance_id], |row| row.get(0))
        .map_err(db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db)?;
    Ok(RuntimeSnapshot {
        instance_id: instance_id.into(),
        sequence,
        inputs: vec![],
        events,
        paused_conversation_ids,
        input_runs: vec![],
        preference_updates: vec![],
        summary: Some(RuntimeSummary {
            pending_by_node,
            conversation_changes,
            structure_revision,
        }),
    })
}

#[cfg(test)]
mod tests;
