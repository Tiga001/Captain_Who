//! Explicit administrator configuration reads; never included in automatic World State.
use super::*;
use crate::organization_personnel::{authorize_target, can_manage};
use crate::storage::{composer_draft_repository, conversation_trace_repository};

pub(super) fn members(
    c: &Connection,
    graph: &Graph,
    identity: &ConversationSnapshot,
    query: &StateQuery,
) -> Result<Value, String> {
    let actor = node(graph, &identity.node_id)?;
    if !can_manage(&actor.management_role, actor.department_id.as_deref()) {
        return Err(
            "organization_configuration_forbidden: Only organization or department administrators can inspect member configuration"
                .into(),
        );
    }
    let readable = |target: &Node| {
        target.id == actor.id
            || authorize_target(
                actor,
                Some(target),
                target.rank,
                target.department_id.as_deref(),
                &graph.definition.departments,
            )
            .is_ok()
    };
    if query.node_id.as_deref().is_some_and(|id| {
        graph
            .definition
            .nodes
            .iter()
            .any(|node| node.id == id && !readable(node))
    }) {
        return Err("organization_configuration_forbidden: Member configuration is outside your administrator scope".into());
    }
    let candidates = selected_nodes(graph, query)?;
    let excluded = candidates.iter().filter(|node| !readable(node)).count();
    let mut members = Vec::new();
    for node in candidates.into_iter().filter(|node| readable(node)) {
        let NodeConfig::Agent(config) = &node.config;
        let chat = graph.bindings.get(&node.id);
        let mut next_turn = Value::Null;
        let mut active_run = Value::Null;
        if let Some(chat) = chat {
            if let Some(draft) =
                composer_draft_repository::get_composer_draft(c, chat).map_err(db)?
            {
                let draft = draft.normalize_permission_mode();
                let model = match draft.model_id {
                    Some(id) => Some(id),
                    None => c
                        .query_row(
                            "SELECT model_id FROM conversations WHERE id=?1",
                            [chat],
                            |row| row.get::<_, Option<String>>(0),
                        )
                        .optional()
                        .map_err(db)?
                        .flatten(),
                };
                next_turn = value!({"modelConfigId":model,"permissionMode":draft.permission_mode});
            }
            if let Some(turn) =
                conversation_trace_repository::get_in_progress_turn_identity(c, chat).map_err(db)?
            {
                active_run = value!({"runId":turn.run_id,"configuration":"frozen_for_this_turn_not_reported_here"});
            }
        }
        members.push(value!({"nodeId":node.id,"nodeName":node.name,"departmentId":node.department_id,"editable":node.id!=actor.id,
            "memberDefaults":{"modelConfigId":config.model_config_id,"permissionMode":config.permission_mode},
            "nextTurn":next_turn,"activeRun":active_run}));
    }
    Ok(
        value!({"members":members,"access":{"scope":"authorized_members_only","excludedMemberCount":excluded},"configurationRule":"memberDefaults are the saved organization settings. nextTurn is the conversation's current composer selection used by the next automatic mail wake (null means not configured). Existing manually queued messages retain their own settings. These are not the running turn's frozen configuration; editing does not change an active turn. Use nextTurn.modelConfigId to copy another member's next-turn model, and only choose IDs from availableModels."}),
    )
}
