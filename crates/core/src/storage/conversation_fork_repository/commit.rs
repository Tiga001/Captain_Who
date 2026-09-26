#[cfg(test)]
pub(crate) fn commit_fork_plan(
    connection: &mut Connection,
    plan: &ConversationForkPlan,
) -> Result<(), ConversationForkError> {
    commit_fork_plan_with_provider_continuations(connection, plan, &[])
}

pub(crate) fn commit_fork_plan_with_provider_continuations(
    connection: &mut Connection,
    plan: &ConversationForkPlan,
    provider_continuations: &[PreparedProviderContinuationClone],
) -> Result<(), ConversationForkError> {
    if provider_continuations.len() != plan.provider_continuation_mappings.len() {
        return Err("Provider continuation 克隆未完整准备，已安全取消整个分叉。"
            .to_string()
            .into());
    }
    for (mapping, prepared) in plan
        .provider_continuation_mappings
        .iter()
        .zip(provider_continuations)
    {
        if prepared.source_ref != mapping.source_ref
            || prepared.target_ref.id != prepared.record.continuation_id
            || prepared.record.conversation_id != mapping.target_conversation_id
            || prepared.record.assistant_message_id != mapping.target_assistant_message_id
            || prepared.record.run_id != mapping.target_run_id
            || prepared.record.request_index != mapping.request_index
        {
            return Err("Provider continuation 克隆身份不匹配，已安全取消整个分叉。"
                .to_string()
                .into());
        }
    }
    let target_message_id = mapped_id(
        &plan.message_id_map,
        &plan.source_message_id,
        "新任务接续边界消息",
    )?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(database_error)?;
    let source_conversation_ids = std::iter::once(plan.source_conversation_id.clone())
        .chain(
            plan.members
                .iter()
                .map(|member| member.source_agent.conversation_id.clone()),
        )
        .collect::<Vec<_>>();
    let source_agent_ids = plan
        .collaboration_root
        .iter()
        .map(|root| root.source_agent_id.clone())
        .chain(
            plan.members
                .iter()
                .map(|member| member.source_agent.agent_id.clone()),
        )
        .collect::<Vec<_>>();
    ensure_agent_tree_stable(&transaction, &source_conversation_ids, &source_agent_ids)?;
    if matches!(plan.source_fork_point, ConversationForkPoint::Latest {}) {
        let source =
            chat_repository::get_active_conversation(&transaction, &plan.source_conversation_id)
                .map_err(database_error)?
                .ok_or_else(|| "原任务已不存在。".to_string())?;
        let head = context_compaction_repository::get_active_summary(&transaction, &source.id)
            .map_err(|error| error.to_string())?;
        if source.messages.last().map(|message| message.id.as_str())
            != Some(plan.source_message_id.as_str())
            || source.model_id != plan.target.model_id
            || head.as_ref().map(|summary| summary.id.as_str())
                != plan
                    .summaries
                    .last()
                    .map(|version| version.summary.id.as_str())
        {
            return Err("最新分支边界已变化，请重试。".to_string().into());
        }
    }
    if matches!(
        plan.source_fork_point,
        ConversationForkPoint::ManualCompactionBoundary { .. }
    ) {
        let source =
            chat_repository::get_active_conversation(&transaction, &plan.source_conversation_id)
                .map_err(database_error)?
                .ok_or_else(|| "原任务已不存在。".to_string())?;
        let chain =
            context_compaction_repository::list_active_summary_chain(&transaction, &source.id)
                .map_err(|error| error.to_string())?;
        let boundary = resolve_fork_point(&transaction, &source, &plan.source_fork_point, &chain)?;
        if boundary.assistant_message_id != plan.source_message_id
            || boundary.model_id != plan.target.model_id
            || boundary.summary_id.as_deref()
                != plan
                    .summaries
                    .last()
                    .map(|version| version.summary.id.as_str())
        {
            return Err("手动压缩分支边界已变化，请重试。".to_string().into());
        }
    }
    insert_conversation(&transaction, &plan.target)?;
    // Workflow provenance is independent of Agent-tree snapshot authorization and survives
    // ordinary standalone forks as well as collaboration-root forks.
    for (source_message_id,target_message_id) in &plan.message_id_map {
        transaction.execute("INSERT INTO workflow_execution_message_origins(message_id,conversation_id,input_id) SELECT ?1,?2,input_id FROM workflow_execution_message_origins WHERE message_id=?3 AND conversation_id=?4",params![target_message_id,plan.target.id,source_message_id,plan.source_conversation_id]).map_err(database_error)?;
    }

    if let Some(collaboration) = &plan.collaboration_root {
        let source = agent_graph_repository::get_agent_node_by_conversation(
            &transaction,
            &plan.source_conversation_id,
        )
        .map_err(|error| ConversationForkError::Other(error.to_string()))?
        .filter(|node| {
            node.agent_id == collaboration.source_agent_id
                && node.parent_agent_id.is_none()
                && node.lifecycle == AgentLifecycle::Active
        })
        .ok_or_else(|| {
            ConversationForkError::Other("根 Agent 身份在分叉提交前已改变，请重试。".to_string())
        })?;
        if source.root_conversation_id != plan.source_conversation_id {
            return Err(ConversationForkError::Other(
                "根 Agent 的 Conversation 身份无效。".to_string(),
            ));
        }
        agent_graph_repository::ensure_root_agent_in_transaction(
            &transaction,
            &collaboration.target_root,
            plan.target.created_at,
        )
        .map_err(|error| ConversationForkError::Other(error.to_string()))?;
    }
    insert_root_fork_receipt(&transaction, plan, &target_message_id)?;
    apply_fork_snapshot_origins(
        &transaction,
        &plan.source_conversation_id,
        &plan.target.id,
        &plan.snapshot_origins,
    )?;

    for member in &plan.members {
        let current_source =
            agent_graph_repository::get_agent_node(&transaction, &member.source_agent.agent_id)
                .map_err(|error| ConversationForkError::Other(error.to_string()))?;
        if current_source.as_ref() != Some(&member.source_agent) {
            return Err(ConversationForkError::Other(
                "成员 Agent 身份在分叉提交前已改变，请重试。".to_string(),
            ));
        }
        insert_empty_conversation(&transaction, &member.history.target)?;
        agent_graph_repository::insert_forked_agent_node_in_transaction(
            &transaction,
            &member.target_agent,
        )
        .map_err(|error| ConversationForkError::Other(error.to_string()))?;
        insert_member_fork_receipt(&transaction, plan, member)?;
        insert_snapshot_messages(&transaction, member.history.as_ref())?;
    }

    for (mapping, prepared) in plan
        .provider_continuation_mappings
        .iter()
        .zip(provider_continuations)
    {
        let outcome = match mapping.source_record.projection {
            Some(projection) => {
                provider_continuation_repository::store_active_with_projection_in_connection(
                    &transaction,
                    &prepared.record,
                    projection,
                )
            }
            None => provider_continuation_repository::store_active_in_connection(
                &transaction,
                &prepared.record,
            ),
        }
        .map_err(database_error)?;
        match outcome {
            provider_continuation_repository::ProviderContinuationStoreOutcome::Inserted {
                ..
            } => {}
            provider_continuation_repository::ProviderContinuationStoreOutcome::Idempotent {
                ..
            }
            | provider_continuation_repository::ProviderContinuationStoreOutcome::Conflict => {
                return Err("Provider continuation 克隆写入冲突，已安全取消整个分叉。"
                    .to_string()
                    .into());
            }
        }
    }
    apply_history_facts(&transaction, plan.root_history_ref())?;
    for member in &plan.members {
        apply_history_facts(&transaction, member.history.as_ref())?;
    }
    settle_forked_member_lifecycles(&transaction, plan)?;
    transaction
        .commit()
        .map_err(database_error)
        .map_err(Into::into)
}
