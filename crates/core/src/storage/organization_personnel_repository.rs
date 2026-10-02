//! Atomic organization edits with Host-owned identity, revision and permission ceilings.
use crate::organization_personnel::{can_manage, Receipt, Request};
use crate::storage::{now_ms, workflow_execution_repository, workflow_repository};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

mod edit;
mod geometry;
#[cfg(test)]
mod tests;
fn error(error: impl std::fmt::Display) -> String {
    format!("Organization edit: {error}")
}

/// Receipt recovery is bound to the original Host run and tool call, not a newly refreshed
/// organization revision or current administrator role. It can only return a committed result.
#[cfg(test)]
pub fn receipt_for_call(
    c: &Connection,
    conversation_id: &str,
    run_id: &str,
    tool_call_id: &str,
    input: &crate::organization_personnel::Input,
) -> Result<Option<Receipt>, String> {
    receipt_for_model_call(
        c,
        conversation_id,
        run_id,
        tool_call_id,
        &serde_json::to_value(input).map_err(error)?,
    )
}

pub fn receipt_for_model_call(
    c: &Connection,
    conversation_id: &str,
    run_id: &str,
    tool_call_id: &str,
    input: &serde_json::Value,
) -> Result<Option<Receipt>, String> {
    let row: Option<(String,String)> = c.query_row("SELECT request_json,receipt_json FROM organization_personnel_receipts WHERE source_run_id=?1 AND tool_call_id=?2",
        params![run_id,tool_call_id],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(error)?;
    let Some((request, receipt)) = row else {
        return Ok(None);
    };
    let request: serde_json::Value = serde_json::from_str(&request).map_err(error)?;
    if request["conversationId"] != conversation_id
        || request["sourceRunId"] != run_id
        || request["toolCallId"] != tool_call_id
        || request
            .get("modelInput")
            .filter(|value| !value.is_null())
            .unwrap_or(&request["input"])
            != input
    {
        return Err(
            "Organization personnel call identity was reused with different arguments".into(),
        );
    }
    serde_json::from_str(&receipt).map(Some).map_err(error)
}

pub fn manage(
    c: &mut Connection,
    request: &Request,
    available_models: &std::collections::HashSet<String>,
) -> Result<Receipt, String> {
    request.input.validate()?;
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error)?;
    // Recovery verifies only the immutable Host call owner and original model arguments. Live
    // revisions, permission presets and membership may have changed after the effect committed.
    if let Some(receipt) = receipt_for_model_call(
        &tx,
        &request.conversation_id,
        &request.source_run_id,
        &request.tool_call_id,
        &request
            .model_input
            .clone()
            .unwrap_or(serde_json::to_value(&request.input).map_err(error)?),
    )? {
        return Ok(receipt);
    }
    let preferences =
        crate::storage::preferences_repository::load_ui_preferences(&tx).map_err(error)?;
    let permission_settings_fingerprint = serde_json::to_string(&(
        preferences.full_permission_enabled,
        preferences.custom_permission_enabled,
        preferences.custom_permissions,
    ))
    .map_err(error)?;
    if request.context.permission_settings_fingerprint != permission_settings_fingerprint {
        return Err(
            "organization_permission_settings_changed: Read current permissions before editing"
                .into(),
        );
    }
    let owner = workflow_execution_repository::snapshot_for_run(
        &tx,
        &request.conversation_id,
        &request.source_run_id,
    )?
    .ok_or("organization_management_denied")?;
    if owner.execution_version != request.execution_version {
        return Err("organization_management_denied".into());
    }
    let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_turn_traces WHERE conversation_id=?1 AND run_id=?2 AND terminal_status='in_progress') AND NOT EXISTS(SELECT 1 FROM workflow_mail_pauses WHERE conversation_id=?1)",params![request.conversation_id,request.source_run_id],|row|row.get(0)).map_err(error)?;
    if !active {
        return Err("Organization changes require the active, unstopped conversation run".into());
    }
    let instance = workflow_repository::load_instance(&tx, &owner.instance_id)
        .map_err(error)?
        .ok_or("organization_management_denied")?;
    if instance.revision != request.expected_revision {
        return Err(
            "organization_revision_conflict: Read the updated organization state before editing"
                .into(),
        );
    }
    let original = instance.definition.clone();
    let actor = original
        .nodes
        .iter()
        .find(|node| node.id == owner.node_id)
        .ok_or("organization_management_denied")?;
    if !can_manage(&actor.management_role, actor.department_id.as_deref()) {
        return Err("organization_management_denied".into());
    }
    let mut definition = original.clone();
    let mut receipt = Receipt {
        instance_id: instance.id.clone(),
        organization_name: instance.name.clone(),
        organization_revision: instance.revision + 1,
        changes: Vec::new(),
        affected_conversation_ids: Vec::new(),
    };
    let mut editor = edit::Editor {
        c: &tx,
        instance: &instance,
        actor,
        context: &request.context,
        available_models,
        original: &original,
        definition: &mut definition,
        receipt: &mut receipt,
    };
    let mut needs_layout = false;
    for action in &request.input.changes {
        needs_layout |= editor.apply(action)?;
    }
    if needs_layout {
        geometry::reconcile(&original, &mut definition)?;
    }
    workflow_repository::save_instance_definition(
        &tx,
        &instance.id,
        &definition,
        request.expected_revision,
    )
    .map_err(error)?;
    receipt.affected_conversation_ids.sort();
    receipt.affected_conversation_ids.dedup();
    let now = now_ms();
    tx.execute("INSERT INTO workflow_mail_events(instance_id,source_node_id,kind,created_at) VALUES(?1,?2,'members_changed',?3)",params![instance.id,owner.node_id,now]).map_err(error)?;
    let request_json=serde_json::to_string(&serde_json::json!({"conversationId":request.conversation_id,"sourceRunId":request.source_run_id,"toolCallId":request.tool_call_id,"input":request.input,"modelInput":request.model_input})).map_err(error)?;
    tx.execute("INSERT INTO organization_personnel_receipts(source_run_id,tool_call_id,request_json,receipt_json,created_at) VALUES(?1,?2,?3,?4,?5)",params![request.source_run_id,request.tool_call_id,request_json,serde_json::to_string(&receipt).map_err(error)?,now]).map_err(error)?;
    tx.commit().map_err(error)?;
    Ok(receipt)
}
