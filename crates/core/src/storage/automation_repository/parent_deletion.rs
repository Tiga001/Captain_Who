// Parent-resource triggers intentionally process only live tasks. A tombstoned task still has
// foreign keys, however, and its healthy-state CHECKs would reject ON DELETE SET NULL. Prepare
// just these hidden records inside the parent's transaction, including trigger-disabled Agent
// tree deletion. Do not publish attention, mutate Run history, or revive scheduling.

pub(crate) fn prepare_tombstoned_automations_for_project_delete(
    transaction: &Transaction<'_>,
    project_id: &str,
    conversation_ids: &[String],
) -> rusqlite::Result<()> {
    prepare_tombstoned_automation_parent(
        transaction,
        DeletedAutomationParent::Project(project_id),
    )?;
    prepare_tombstoned_automations_for_conversation_delete(transaction, conversation_ids)
}

pub(crate) fn prepare_tombstoned_automations_for_conversation_delete(
    transaction: &Transaction<'_>,
    conversation_ids: &[String],
) -> rusqlite::Result<()> {
    for conversation_id in conversation_ids {
        prepare_tombstoned_automation_parent(
            transaction,
            DeletedAutomationParent::Conversation(conversation_id),
        )?;
    }
    Ok(())
}

pub(crate) fn prepare_tombstoned_automations_for_model_delete(
    transaction: &Transaction<'_>,
    model_id: &str,
) -> rusqlite::Result<()> {
    prepare_tombstoned_automation_parent(transaction, DeletedAutomationParent::Model(model_id))
}

enum DeletedAutomationParent<'a> {
    Project(&'a str),
    Conversation(&'a str),
    Model(&'a str),
}

fn prepare_tombstoned_automation_parent(
    transaction: &Transaction<'_>,
    parent: DeletedAutomationParent<'_>,
) -> rusqlite::Result<()> {
    // Every SQL identifier comes from this closed mapping; resource IDs remain bound parameters.
    let (id, table, name_column, reference, snapshot, code, message) = match parent {
        DeletedAutomationParent::Project(id) => (
            id,
            "projects",
            "name",
            "project_id",
            "target_project",
            "project_missing",
            "The target project no longer exists.",
        ),
        DeletedAutomationParent::Conversation(id) => (
            id,
            "conversations",
            "title",
            "target_conversation_id",
            "target_conversation",
            "target_missing",
            "The target conversation no longer exists.",
        ),
        DeletedAutomationParent::Model(id) => (
            id,
            "models",
            "display_name",
            "model_id",
            "target_model",
            "model_missing",
            "The selected model no longer exists.",
        ),
    };
    let name = transaction
        .query_row(
            &format!("SELECT {name_column} FROM {table} WHERE id = ?1"),
            [id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(name) = name else {
        return Ok(());
    };
    // Project names and conversation titles do not share the snapshot's 512-byte bound.
    let name = truncate_utf8(name.trim(), 512);
    let name = (!name.is_empty()).then_some(name);
    transaction.execute(
        &format!(
            "UPDATE automations
             SET health_state = 'blocked',
                 blocked_code = COALESCE(blocked_code, ?2),
                 blocked_message = COALESCE(blocked_message, ?3),
                 {snapshot}_snapshot = COALESCE({snapshot}_snapshot, ?4),
                 {snapshot}_id_snapshot = COALESCE({snapshot}_id_snapshot, ?1)
             WHERE deleted_at IS NOT NULL AND {reference} = ?1"
        ),
        params![id, code, message, name],
    )?;
    // Keep the tombstone, revision, timestamps, attention, and schedule as recorded at deletion.
    // The subsequent parent DELETE clears only the relevant live FK; snapshots remain history.
    Ok(())
}
