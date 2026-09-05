//! Display provenance is immutable history, separate from resumable/deliverable answer authority.
use super::*;

pub(crate) fn store_message_projection(
    connection: &Connection,
    message_id: &str,
    display: &HumanInteractionResponseDisplay,
) -> Result<()> {
    display.validate()?;
    let content = json(display)?;
    let old: Option<String> = connection
        .query_row(
            "SELECT content_json FROM human_interaction_message_projections WHERE message_id=?1",
            [message_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(unavailable)?;
    if let Some(old) = old {
        return if old == content {
            Ok(())
        } else {
            Err(conflict())
        };
    }
    connection.execute(
        "INSERT INTO human_interaction_message_projections(message_id,request_id,response_id,content_json) VALUES(?1,?2,?3,?4)",
        params![message_id,display.request_id,display.response_id,content],
    ).map_err(unavailable)?;
    Ok(())
}

pub(crate) fn attach_message_projections(
    connection: &Connection,
    conversation_id: &str,
    messages: &mut [crate::storage::models::ChatMessageRecord],
) -> rusqlite::Result<()> {
    let mut statement = connection.prepare(
        "SELECT p.message_id,p.request_id,p.response_id,p.content_json FROM human_interaction_message_projections p JOIN messages m ON m.id=p.message_id WHERE m.conversation_id=?1",
    )?;
    let records = statement.query_map([conversation_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    // Incoming structs never grant provenance: attach only records read from the native journal.
    let indexes: std::collections::HashMap<_, _> = messages
        .iter()
        .enumerate()
        .map(|(index, message)| (message.id.clone(), index))
        .collect();
    for message in messages.iter_mut() {
        message.human_interaction_response = None;
    }
    for record in records {
        let (id, request_id, response_id, content) = record?;
        let Some(index) = indexes.get(&id) else {
            continue;
        };
        let message = &mut messages[*index];
        let display: HumanInteractionResponseDisplay =
            serde_json::from_str(&content).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
        if display.validate().is_err()
            || display.request_id != request_id
            || display.response_id != response_id
            || message.role != "user"
            || message.content != content
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        message.human_interaction_response = Some(display);
    }
    Ok(())
}
