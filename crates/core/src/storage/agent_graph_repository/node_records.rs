use super::common::{corrupt, decode_bool, positive_u64, read_error, validate_schema_version};
use crate::{
    AgentGraphError, AgentLifecycle, AgentModelSelectionSnapshot, AgentModelSelectionSource,
    AgentNodeRecord, AgentTemplateSnapshot, ReasoningEffort,
};
use rusqlite::{params, Connection, OptionalExtension};

pub(super) const NODE_SELECT: &str = "
    SELECT agent_id, schema_version, root_agent_id, root_conversation_id,
           parent_agent_id, conversation_id, project_id, creation_request_id,
           task_name, task_path,
           template_id_snapshot, template_project_id_snapshot,
           template_machine_key_snapshot, template_name_snapshot,
           template_description_snapshot, template_instructions_snapshot,
           template_revision_snapshot, template_model_config_id_snapshot,
           model_config_id_snapshot, model_display_name_snapshot,
           model_supports_image_snapshot, model_context_window_tokens_snapshot,
           model_settings_revision_snapshot, provider_connection_revision_snapshot,
           provider_protocol_revision_snapshot, model_selection_source_snapshot,
           reasoning_effort_snapshot, lifecycle, revision, created_at, updated_at
    FROM agent_nodes";

pub(super) fn query_node(
    connection: &Connection,
    agent_id: &str,
) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{NODE_SELECT} WHERE agent_id = ?1"),
            [agent_id],
            read_node_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_node)
        .transpose()
}

pub(super) fn query_node_by_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{NODE_SELECT} WHERE conversation_id = ?1"),
            [conversation_id],
            read_node_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_node)
        .transpose()
}

pub(super) fn query_node_by_root_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{NODE_SELECT} WHERE root_conversation_id = ?1 AND parent_agent_id IS NULL"),
            [conversation_id],
            read_node_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_node)
        .transpose()
}

pub(super) fn query_node_by_request(
    connection: &Connection,
    root_agent_id: &str,
    request_id: &str,
) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{NODE_SELECT} WHERE root_agent_id = ?1 AND creation_request_id = ?2"),
            params![root_agent_id, request_id],
            read_node_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_node)
        .transpose()
}

pub(super) fn query_nodes<P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<AgentNodeRecord>, AgentGraphError> {
    let mut statement = connection.prepare(sql).map_err(read_error)?;
    let rows = statement
        .query_map(params, read_node_row)
        .map_err(read_error)?;
    rows.map(|row| row.map_err(read_error).and_then(decode_node))
        .collect()
}

#[allow(clippy::type_complexity)]
pub(super) fn read_node_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NodeRow> {
    Ok(NodeRow {
        agent_id: row.get(0)?,
        schema_version: row.get(1)?,
        root_agent_id: row.get(2)?,
        root_conversation_id: row.get(3)?,
        parent_agent_id: row.get(4)?,
        conversation_id: row.get(5)?,
        project_id: row.get(6)?,
        creation_request_id: row.get(7)?,
        task_name: row.get(8)?,
        task_path: row.get(9)?,
        template_id: row.get(10)?,
        template_project_id: row.get(11)?,
        template_machine_key: row.get(12)?,
        template_name: row.get(13)?,
        template_description: row.get(14)?,
        template_instructions: row.get(15)?,
        template_revision: row.get(16)?,
        template_model_config_id: row.get(17)?,
        model_config_id: row.get(18)?,
        model_display_name: row.get(19)?,
        model_supports_image: row.get(20)?,
        model_context_window: row.get(21)?,
        model_settings_revision: row.get(22)?,
        provider_connection_revision: row.get(23)?,
        provider_protocol_revision: row.get(24)?,
        model_selection_source: row.get(25)?,
        reasoning_effort_snapshot: row.get(26)?,
        lifecycle: row.get(27)?,
        revision: row.get(28)?,
        created_at: row.get(29)?,
        updated_at: row.get(30)?,
    })
}

pub(super) struct NodeRow {
    agent_id: String,
    schema_version: i64,
    root_agent_id: String,
    root_conversation_id: String,
    parent_agent_id: Option<String>,
    conversation_id: String,
    project_id: Option<String>,
    creation_request_id: String,
    task_name: String,
    task_path: String,
    template_id: Option<String>,
    template_project_id: Option<String>,
    template_machine_key: Option<String>,
    template_name: Option<String>,
    template_description: Option<String>,
    template_instructions: Option<String>,
    template_revision: Option<i64>,
    template_model_config_id: Option<String>,
    model_config_id: Option<String>,
    model_display_name: Option<String>,
    model_supports_image: Option<i64>,
    model_context_window: Option<i64>,
    model_settings_revision: Option<String>,
    provider_connection_revision: Option<String>,
    provider_protocol_revision: Option<String>,
    model_selection_source: Option<String>,
    reasoning_effort_snapshot: Option<String>,
    lifecycle: String,
    revision: i64,
    created_at: i64,
    updated_at: i64,
}

pub(super) fn decode_node(row: NodeRow) -> Result<AgentNodeRecord, AgentGraphError> {
    validate_schema_version(row.schema_version)?;
    let template_values = (
        row.template_id,
        row.template_project_id,
        row.template_machine_key,
        row.template_name,
        row.template_description,
        row.template_instructions,
        row.template_revision,
        row.template_model_config_id,
    );
    let template_snapshot = match template_values {
        (None, None, None, None, None, None, None, None) => None,
        (
            Some(template_id),
            Some(project_id),
            Some(machine_key),
            Some(name),
            Some(description),
            Some(instructions),
            Some(revision),
            Some(model_config_id),
        ) => Some(AgentTemplateSnapshot {
            template_id,
            project_id,
            machine_key,
            name,
            description,
            instructions,
            template_revision: positive_u64(revision, "template revision")?,
            model_config_id,
        }),
        _ => return Err(corrupt("Agent template snapshot is partial")),
    };
    let model_values = (
        row.model_config_id,
        row.model_display_name,
        row.model_supports_image,
        row.model_context_window,
        row.model_settings_revision,
        row.provider_connection_revision,
        row.provider_protocol_revision,
    );
    let model_snapshot = match model_values {
        (None, None, None, None, None, None, None) => None,
        (
            Some(model_config_id),
            Some(display_name),
            Some(supports_image),
            Some(context_window),
            Some(settings_revision),
            Some(connection_revision),
            Some(protocol_revision),
        ) => Some(AgentModelSelectionSnapshot {
            model_config_id,
            display_name,
            supports_image: decode_bool(supports_image, "model supports image")?,
            effective_context_window_tokens: u32::try_from(context_window)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| corrupt("model context window is invalid"))?,
            model_settings_configuration_revision: settings_revision,
            provider_connection_revision: connection_revision,
            provider_protocol_revision: protocol_revision,
        }),
        _ => return Err(corrupt("Agent model snapshot is partial")),
    };
    let model_selection_source = row
        .model_selection_source
        .as_deref()
        .map(crate::AgentModelSelectionSource::parse)
        .transpose()?;
    let reasoning_effort_snapshot = row
        .reasoning_effort_snapshot
        .as_deref()
        .map(reasoning_effort_from_str)
        .transpose()?;
    match (
        row.parent_agent_id.is_some(),
        model_selection_source,
        template_snapshot.as_ref(),
        model_snapshot.as_ref(),
    ) {
        (false, None, None, None) => {}
        (true, Some(AgentModelSelectionSource::Explicit), _, Some(_)) => {}
        (true, Some(AgentModelSelectionSource::Template), Some(template), Some(model))
            if template.model_config_id == model.model_config_id => {}
        (
            true,
            Some(AgentModelSelectionSource::Parent | AgentModelSelectionSource::Default),
            None,
            Some(_),
        ) => {}
        _ => return Err(corrupt("Agent model selector provenance is inconsistent")),
    }
    if row.parent_agent_id.is_none() && reasoning_effort_snapshot.is_some() {
        return Err(corrupt("root Agent cannot freeze a reasoning effort"));
    }
    Ok(AgentNodeRecord {
        agent_id: row.agent_id,
        root_agent_id: row.root_agent_id,
        root_conversation_id: row.root_conversation_id,
        parent_agent_id: row.parent_agent_id,
        conversation_id: row.conversation_id,
        project_id: row.project_id,
        creation_request_id: row.creation_request_id,
        task_name: row.task_name,
        task_path: row.task_path,
        template_snapshot,
        model_snapshot,
        model_selection_source,
        reasoning_effort_snapshot,
        lifecycle: AgentLifecycle::parse(&row.lifecycle)?,
        revision: positive_u64(row.revision, "Agent revision")?,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

pub(super) fn reasoning_effort_as_str(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::High => "high",
        ReasoningEffort::Max => "max",
        ReasoningEffort::ProviderDefault => "provider_default",
    }
}

pub(super) fn reasoning_effort_from_str(value: &str) -> Result<ReasoningEffort, AgentGraphError> {
    match value {
        "high" => Ok(ReasoningEffort::High),
        "max" => Ok(ReasoningEffort::Max),
        _ => Err(corrupt("Agent reasoning effort snapshot is invalid")),
    }
}
