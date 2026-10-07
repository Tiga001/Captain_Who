//! Automatic requests consume only their identity and mailbox. Explicit state tools retain
//! the separate full member/runtime projection. This read neither claims nor acknowledges mail.
use super::*;

pub fn request_observation(
    c: &Connection,
    conversation_id: &str,
    run_id: Option<&str>,
) -> Result<Option<(ConversationSnapshot, Value)>, String> {
    let tx = c.unchecked_transaction().map_err(db)?;
    let identity = match run_id {
        Some(run_id) => snapshot_for_run(&tx, conversation_id, run_id)?,
        None => snapshot_for_conversation(&tx, conversation_id)?,
    };
    let Some(identity) = identity else {
        return Ok(None);
    };
    // The validated identity already proves this exact conversation owns the current member.
    // Do not rebuild its graph or query the other members just to obtain two mailbox counts.
    let (latest, received, pending, processing): (u64, u64, u64, u64) = tx
        .query_row(
            "SELECT COALESCE(MAX(sequence),0),COUNT(*),
                COALESCE(SUM(mail_status='pending'),0),COALESCE(SUM(mail_status='processing'),0)
         FROM workflow_mail_messages
         WHERE instance_id=?1 AND node_id=?2 AND recipient_conversation_id=?3",
            params![identity.instance_id, identity.node_id, conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(db)?;
    let mut statement = tx
        .prepare(
            "SELECT sequence,message_id,json_extract(message_json,'$.sourceNodeId'),
                json_extract(message_json,'$.sourceNodeName')
         FROM workflow_mail_messages
         WHERE instance_id=?1 AND node_id=?2 AND recipient_conversation_id=?3
         ORDER BY sequence DESC LIMIT 5",
        )
        .map_err(db)?;
    let arrivals = statement
        .query_map(
            params![identity.instance_id, identity.node_id, conversation_id],
            |row| {
                Ok(
                    value!({"sequence":row.get::<_,u64>(0)?,"messageId":row.get::<_,String>(1)?,
            "sourceNodeId":row.get::<_,String>(2)?,"sourceNodeName":row.get::<_,String>(3)?}),
                )
            },
        )
        .map_err(db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db)?;
    let observation = value!({"available":true,"instanceId":identity.instance_id,
        "executionVersion":identity.execution_version,"organizationRevision":identity.organization_revision,
        "currentNodeId":identity.node_id,"mailbox":{"latestSequence":latest,"receivedCount":received,
            "pendingCount":pending,"processingCount":processing,"recentArrivals":arrivals}});
    Ok(Some((identity, observation)))
}
