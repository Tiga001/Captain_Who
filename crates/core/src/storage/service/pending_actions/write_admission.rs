use crate::storage::{write_size::estimated_json_bytes, StorageWriteClass};

// Admission accounts for the caller-retained payload without allocating encoded copies. These
// estimates bound queued inputs; transaction output/SQLite cache memory is measured separately.
fn pending_text_bytes(parts: &[&str]) -> usize {
    parts.iter().fold(512_usize, |bytes, part| {
        bytes.saturating_add(part.len().saturating_mul(2))
    })
}
fn pending_record_bytes(record: &AgentPendingActionRecord) -> usize {
    pending_text_bytes(&[
        &record.action_id,
        &record.run_id,
        &record.action_type,
        &record.tool_name,
        &record.status,
        &record.action_json,
        &record.agent_input_json,
        record.conversation_id.as_deref().unwrap_or_default(),
        record.assistant_message_id.as_deref().unwrap_or_default(),
        record.tool_call_id.as_deref().unwrap_or_default(),
        record.target_status.as_deref().unwrap_or_default(),
    ])
}
fn pending_audit_bytes(record: &AgentActionAuditRecord) -> usize {
    pending_text_bytes(&[
        &record.action_id,
        &record.run_id,
        &record.action_type,
        &record.tool_name,
        &record.status,
        &record.action_json,
        record.conversation_id.as_deref().unwrap_or_default(),
        record.assistant_message_id.as_deref().unwrap_or_default(),
        record.decision.as_deref().unwrap_or_default(),
        record
            .file_change_result_json
            .as_deref()
            .unwrap_or_default(),
        record.command_result_json.as_deref().unwrap_or_default(),
        record.tool_result_json.as_deref().unwrap_or_default(),
        record.error.as_deref().unwrap_or_default(),
        record
            .effective_permissions_json
            .as_deref()
            .unwrap_or_default(),
        record.path_scope.as_deref().unwrap_or_default(),
        record.command_cwd_scope.as_deref().unwrap_or_default(),
        record.blocked_reason.as_deref().unwrap_or_default(),
        record.decision_source.as_deref().unwrap_or_default(),
    ])
}
fn pending_notification_bytes(
    record: &notification_repository::NewNotificationEventRecord,
) -> usize {
    pending_text_bytes(&[
        &record.notification_kind,
        &record.source_kind,
        &record.source_id,
        &record.subject_kind,
        &record.subject_text,
        &record.dedupe_key,
        &record.supersession_key,
        record.run_id.as_deref().unwrap_or_default(),
        record.automation_id.as_deref().unwrap_or_default(),
        record.conversation_id.as_deref().unwrap_or_default(),
        record.user_message_id.as_deref().unwrap_or_default(),
        record.assistant_message_id.as_deref().unwrap_or_default(),
        record.approval_action_id.as_deref().unwrap_or_default(),
    ])
}
fn pending_usage_bytes(usage: &AgentUsageRecordInsert) -> usize {
    pending_text_bytes(&[
        &usage.id,
        &usage.conversation_id,
        &usage.message_id,
        &usage.run_id,
        &usage.model_id,
        &usage.model_name,
        usage.project_id.as_deref().unwrap_or_default(),
        usage.status.as_deref().unwrap_or_default(),
        usage.error.as_deref().unwrap_or_default(),
        usage.input_price.as_deref().unwrap_or_default(),
        usage.cached_input_price.as_deref().unwrap_or_default(),
        usage.output_price.as_deref().unwrap_or_default(),
    ])
}
