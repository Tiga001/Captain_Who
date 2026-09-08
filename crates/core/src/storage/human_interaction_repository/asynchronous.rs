//! Native Host admission and delivery facts for nonblocking question batches.
use super::*;

/// One idempotent asynchronous batch per exact Host-owned tool call. Existing batches retain
/// their original settings revision and questions after settings or the producing run change.
pub fn admit_async(
    connection: &mut Connection,
    owner: &HostHumanInteractionOwner,
    input: &HumanInteractionToolInput,
    now: i64,
) -> Result<HumanInteractionRequestSnapshot> {
    validate_human_interaction_tool_input(input)?;
    valid_time(now)?;
    for id in [
        &owner.agent_id,
        &owner.conversation_id,
        &owner.run_id,
        &owner.assistant_message_id,
        &owner.tool_call_id,
    ] {
        validate_human_interaction_id(id)?;
    }
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let existing: Option<String> = tx
        .query_row(
            "SELECT request_id FROM human_interaction_requests WHERE run_id=?1 AND tool_call_id=?2",
            params![owner.run_id, owner.tool_call_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(unavailable)?;
    if let Some(id) = existing {
        let old = load_request(&tx, &owner.conversation_id, &id)?;
        let agent: String = tx
            .query_row(
                "SELECT agent_id FROM human_interaction_requests WHERE request_id=?1",
                [&id],
                |r| r.get(0),
            )
            .map_err(unavailable)?;
        if old.mode != HumanInteractionMode::Async
            || old.assistant_message_id != owner.assistant_message_id
            || agent != owner.agent_id
            || old
                .questions
                .iter()
                .map(|q| HumanInteractionQuestionInput {
                    title: q.title.clone(),
                    options: q
                        .options
                        .as_ref()
                        .map(|options| options.iter().map(|o| o.label.clone()).collect()),
                })
                .collect::<Vec<_>>()
                != input.questions
        {
            return Err(conflict());
        }
        tx.commit().map_err(unavailable)?;
        return Ok(old);
    }
    let result =
        create_request_in_transaction(&tx, owner, HumanInteractionMode::Async, input, now)?;
    tx.commit().map_err(unavailable)?;
    Ok(result)
}

#[derive(Debug, Clone)]
pub struct HumanInteractionAsyncPending {
    pub request: HumanInteractionRequestSnapshot,
    /// Reuse the original answer bubble after a definitely unstarted Turn is retired.
    pub user_message_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HumanInteractionAsyncTurnAdmission {
    pub response_id: String,
    pub expected_delivery_revision: u64,
    pub user_message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanInteractionAsyncBinding {
    pub request_id: String,
    pub response_id: String,
    pub conversation_id: String,
    pub target_run_id: String,
    pub assistant_message_id: String,
    pub user_message_id: Option<String>,
    pub guidance_id: Option<String>,
    pub claim_id: String,
}

/// The same self-contained answer material is used by idle User messages and active Guidance.
/// Its route comes only from Host delivery facts, never from this display-only JSON envelope.
pub fn async_human_interaction_answer_content(
    request: &HumanInteractionRequestSnapshot,
) -> Result<String> {
    if request.mode != HumanInteractionMode::Async {
        return Err(conflict());
    }
    json(&human_interaction_answer_display(request)?)
}

/// Builds complete immutable display material from accepted Host facts for either delivery mode.
pub fn human_interaction_answer_display(
    request: &HumanInteractionRequestSnapshot,
) -> Result<HumanInteractionResponseDisplay> {
    if request.status != HumanInteractionRequestStatus::Submitted {
        return Err(conflict());
    }
    let response = request.response.as_ref().ok_or_else(conflict)?;
    let ordered = validate_human_interaction_answers(&request.questions, &response.answers)?;
    let answers = request
        .questions
        .iter()
        .zip(ordered)
        .map(|(question, answer)| {
            Ok(match answer {
                HumanInteractionAnswer::Option {
                    question_id,
                    option_id,
                } => {
                    let option = question
                        .options
                        .as_ref()
                        .and_then(|options| options.iter().find(|option| option.id == option_id))
                        .ok_or_else(HumanInteractionError::invalid)?;
                    HumanInteractionAnswerDisplay::Option {
                        question_id,
                        question: question.title.clone(),
                        option_id,
                        answer: option.label.clone(),
                    }
                }
                HumanInteractionAnswer::Text { question_id, text } => {
                    HumanInteractionAnswerDisplay::Text {
                        question_id,
                        question: question.title.clone(),
                        answer: text,
                    }
                }
                HumanInteractionAnswer::Skipped { question_id } => {
                    HumanInteractionAnswerDisplay::Skipped {
                        question_id,
                        question: question.title.clone(),
                        answer: "已跳过".into(),
                    }
                }
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let display = HumanInteractionResponseDisplay {
        result_type: "human_interaction_response".into(),
        schema_version: HUMAN_INTERACTION_SCHEMA_VERSION,
        request_id: request.request_id.clone(),
        response_id: response.response_id.clone(),
        answers,
    };
    display.validate()?;
    Ok(display)
}

pub fn list_pending_async(connection: &Connection) -> Result<Vec<HumanInteractionAsyncPending>> {
    let mut statement = connection.prepare("SELECT r.conversation_id,r.request_id,b.user_message_id FROM human_interaction_requests r JOIN human_interaction_responses a ON a.request_id=r.request_id JOIN human_interaction_deliveries d ON d.response_id=a.response_id JOIN human_interaction_async_bindings b ON b.response_id=a.response_id WHERE r.mode='async' AND r.status='submitted' AND d.status='pending' AND b.status='pending' ORDER BY a.sequence").map_err(unavailable)?;
    let ids = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(unavailable)?;
    ids.into_iter()
        .map(|(conversation, id, user_message_id)| {
            Ok(HumanInteractionAsyncPending {
                request: load_request(connection, &conversation, &id)?,
                user_message_id,
            })
        })
        .collect()
}

pub fn load_async_binding_for_run(
    connection: &Connection,
    run: &str,
) -> Result<Option<HumanInteractionAsyncBinding>> {
    let response: Option<String> = connection.query_row("SELECT response_id FROM human_interaction_async_bindings WHERE target_run_id=?1 AND route='new_turn'",[run],|r|r.get(0)).optional().map_err(unavailable)?;
    response
        .map(|id| load_async_binding(connection, &id))
        .transpose()
        .map(Option::flatten)
}

pub fn load_async_binding(
    connection: &Connection,
    response_id: &str,
) -> Result<Option<HumanInteractionAsyncBinding>> {
    connection.query_row("SELECT r.request_id,b.response_id,r.conversation_id,b.target_run_id,b.assistant_message_id,b.user_message_id,b.guidance_id,b.claim_id FROM human_interaction_async_bindings b JOIN human_interaction_responses a ON a.response_id=b.response_id JOIN human_interaction_requests r ON r.request_id=a.request_id WHERE b.response_id=?1 AND b.route IS NOT NULL AND b.target_run_id IS NOT NULL AND b.assistant_message_id IS NOT NULL AND b.claim_id IS NOT NULL",[response_id],|r|Ok(HumanInteractionAsyncBinding {request_id:r.get(0)?,response_id:r.get(1)?,conversation_id:r.get(2)?,target_run_id:r.get(3)?,assistant_message_id:r.get(4)?,user_message_id:r.get(5)?,guidance_id:r.get(6)?,claim_id:r.get(7)?})).optional().map_err(unavailable)
}

fn pending_async_in_transaction(
    connection: &Connection,
    response_id: &str,
    revision: u64,
) -> Result<Option<HumanInteractionRequestSnapshot>> {
    safe_revision(revision)?;
    let row: Option<(String,String)> = connection.query_row("SELECT r.conversation_id,r.request_id FROM human_interaction_requests r JOIN human_interaction_responses a ON a.request_id=r.request_id JOIN human_interaction_deliveries d ON d.response_id=a.response_id JOIN human_interaction_async_bindings b ON b.response_id=a.response_id WHERE a.response_id=?1 AND r.mode='async' AND r.status='submitted' AND d.status='pending' AND d.revision=?2 AND b.status='pending' AND NOT EXISTS(SELECT 1 FROM agent_tree_run_stops s WHERE s.run_id=b.stop_scope_run_id) AND NOT EXISTS(SELECT 1 FROM human_interaction_responses earlier JOIN human_interaction_requests er ON er.request_id=earlier.request_id JOIN human_interaction_deliveries ed ON ed.response_id=earlier.response_id WHERE er.conversation_id=r.conversation_id AND er.mode='async' AND earlier.sequence<a.sequence AND ed.status='pending')",params![response_id,revision],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(unavailable)?;
    row.map(|(conversation, id)| load_request(connection, &conversation, &id))
        .transpose()
}

fn async_route_blocked(
    connection: &Connection,
    conversation: &str,
    guidance: bool,
) -> Result<bool> {
    // Approval pauses sampling inside the existing Run. Its guidance inbox can retain an answer;
    // starting a new Turn must still wait for every pending/approved/executing action to settle.
    connection.query_row("SELECT EXISTS(SELECT 1 FROM agent_pending_actions WHERE conversation_id=?1 AND status IN ('pending','approved','executing') AND NOT ?2) OR EXISTS(SELECT 1 FROM human_interaction_suspensions s JOIN human_interaction_requests r ON r.request_id=s.request_id WHERE r.conversation_id=?1 AND s.status IN ('waiting','claimed')) OR EXISTS(SELECT 1 FROM manual_context_compaction_operations WHERE conversation_id=?1 AND status='running')",params![conversation,guidance],|r|r.get(0)).map_err(unavailable)
}

pub fn bind_async_to_guidance(
    connection: &mut Connection,
    response_id: &str,
    record: &crate::storage::models::AgentRunGuidanceRecord,
    revision: u64,
    now: i64,
) -> Result<Option<HumanInteractionAsyncBinding>> {
    valid_time(now)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let Some(request) = pending_async_in_transaction(&tx, response_id, revision)? else {
        return Ok(None);
    };
    if record.conversation_id != request.conversation_id
        || record.content != async_human_interaction_answer_content(&request)?
        || !record.attachment_ids.is_empty()
        || async_route_blocked(&tx, &request.conversation_id, true)?
    {
        return Ok(None);
    }
    let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_turn_traces t JOIN agent_nodes n ON n.conversation_id=t.conversation_id WHERE t.run_id=?1 AND t.assistant_message_id=?2 AND t.conversation_id=?3 AND t.terminal_status='in_progress' AND n.parent_agent_id IS NULL AND n.lifecycle='active' AND NOT EXISTS(SELECT 1 FROM agent_tree_run_stops s WHERE s.run_id=t.run_id))",params![record.run_id,record.assistant_message_id,record.conversation_id],|r|r.get(0)).map_err(unavailable)?;
    if !active || !async_predecessors_target(&tx, response_id, &record.run_id)? {
        return Ok(None);
    }
    match crate::storage::guidance_repository::store_guidance_in_connection(&tx, record)
        .map_err(unavailable)?
    {
        crate::storage::guidance_repository::AgentRunGuidanceStoreOutcome::Inserted => {}
        _ => return Err(conflict()),
    }
    let claim_id = Uuid::new_v4().to_string();
    tx.execute("UPDATE human_interaction_async_bindings SET route='guidance',status='bound',claim_id=?1,target_run_id=?2,assistant_message_id=?3,guidance_id=?4,updated_at=MAX(updated_at,?5) WHERE response_id=?6 AND status='pending'",params![claim_id,record.run_id,record.assistant_message_id,record.guidance_id,now,response_id]).map_err(unavailable)?;
    tx.execute("UPDATE human_interaction_deliveries SET status='bound',revision=revision+1,target_run_id=?1 WHERE response_id=?2 AND status='pending'",params![record.run_id,response_id]).map_err(unavailable)?;
    let binding = load_async_binding(&tx, response_id)?.ok_or_else(conflict)?;
    tx.commit().map_err(unavailable)?;
    Ok(Some(binding))
}

/// Called by the existing Turn admission transaction after its conversation/trace writes.
pub(crate) fn bind_async_new_turn_in_transaction(
    connection: &Connection,
    conversation: &crate::storage::models::ChatConversationRecord,
    trace: &crate::ConversationTurnTrace,
    admission: &HumanInteractionAsyncTurnAdmission,
    now: i64,
) -> Result<HumanInteractionAsyncBinding> {
    let request = pending_async_in_transaction(
        connection,
        &admission.response_id,
        admission.expected_delivery_revision,
    )?
    .ok_or_else(conflict)?;
    if request.conversation_id != conversation.id
        || trace.conversation_id != conversation.id
        || async_route_blocked(connection, &conversation.id, false)?
        || !async_predecessors_target(connection, &admission.response_id, &trace.run_id)?
    {
        return Err(conflict());
    }
    let user = conversation
        .messages
        .iter()
        .find(|m| m.id == admission.user_message_id && m.role == "user")
        .ok_or_else(conflict)?;
    if user.content != async_human_interaction_answer_content(&request)?
        || !user.attachments.is_empty()
    {
        return Err(conflict());
    }
    let previous_user: Option<String> = connection
        .query_row(
            "SELECT user_message_id FROM human_interaction_async_bindings WHERE response_id=?1",
            [&admission.response_id],
            |r| r.get(0),
        )
        .map_err(unavailable)?;
    if previous_user
        .as_ref()
        .is_some_and(|id| id != &admission.user_message_id)
    {
        return Err(conflict());
    }
    let claim_id = Uuid::new_v4().to_string();
    connection.execute("UPDATE human_interaction_async_bindings SET route='new_turn',status='bound',claim_id=?1,target_run_id=?2,assistant_message_id=?3,user_message_id=?4,guidance_id=NULL,updated_at=MAX(updated_at,?5) WHERE response_id=?6 AND status='pending'",params![claim_id,trace.run_id,trace.assistant_message_id,admission.user_message_id,now,admission.response_id]).map_err(unavailable)?;
    connection.execute("UPDATE human_interaction_deliveries SET status='bound',revision=revision+1,target_run_id=?1,user_message_id=?2 WHERE response_id=?3 AND status='pending'",params![trace.run_id,admission.user_message_id,admission.response_id]).map_err(unavailable)?;
    store_message_projection(
        connection,
        &admission.user_message_id,
        &human_interaction_answer_display(&request)?,
    )?;
    load_async_binding(connection, &admission.response_id)?.ok_or_else(conflict)
}

fn async_predecessors_target(
    connection: &Connection,
    response_id: &str,
    target: &str,
) -> Result<bool> {
    connection.query_row("SELECT NOT EXISTS(SELECT 1 FROM human_interaction_responses a JOIN human_interaction_requests r ON r.request_id=a.request_id JOIN human_interaction_deliveries d ON d.response_id=a.response_id WHERE r.mode='async' AND r.conversation_id=(SELECT r2.conversation_id FROM human_interaction_requests r2 JOIN human_interaction_responses a2 ON a2.request_id=r2.request_id WHERE a2.response_id=?1) AND a.sequence<(SELECT sequence FROM human_interaction_responses WHERE response_id=?1) AND d.status='bound' AND d.target_run_id IS NOT ?2)",params![response_id,target],|r|r.get(0)).map_err(unavailable)
}

mod recovery;
pub use recovery::*;

/// A retry may reuse only its own previously reserved immutable User identity. Run this before
/// the existing conversation writer so a conflicting message is never overwritten first.
pub(crate) fn validate_async_new_turn_user_in_transaction(
    connection: &Connection,
    conversation: &crate::storage::models::ChatConversationRecord,
    trace: &crate::ConversationTurnTrace,
    admission: &HumanInteractionAsyncTurnAdmission,
) -> Result<()> {
    validate_human_interaction_id(&admission.user_message_id)?;
    let request = pending_async_in_transaction(
        connection,
        &admission.response_id,
        admission.expected_delivery_revision,
    )?
    .ok_or_else(conflict)?;
    if request.conversation_id != conversation.id
        || trace.terminal_status != crate::ConversationTurnTraceTerminalStatus::InProgress
        || !trace.items.is_empty()
    {
        return Err(conflict());
    }
    let previous: Option<String> = connection
        .query_row(
            "SELECT user_message_id FROM human_interaction_async_bindings WHERE response_id=?1",
            [&admission.response_id],
            |r| r.get(0),
        )
        .map_err(unavailable)?;
    if previous
        .as_ref()
        .is_some_and(|id| id != &admission.user_message_id)
    {
        return Err(conflict());
    }
    let existing: Option<(String, String, String)> = connection
        .query_row(
            "SELECT conversation_id,role,content FROM messages WHERE id=?1",
            [&admission.user_message_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(unavailable)?;
    if let Some((id, role, content)) = existing {
        if previous.as_deref() != Some(admission.user_message_id.as_str())
            || id != request.conversation_id
            || role != "user"
            || content != async_human_interaction_answer_content(&request)?
        {
            return Err(conflict());
        }
    }
    Ok(())
}
