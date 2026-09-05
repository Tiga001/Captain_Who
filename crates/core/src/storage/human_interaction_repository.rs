//! Transactional human-question facts. Only Host code may create requests; renderer APIs may
//! read settings/requests and settle a complete batch. Delivery and resumption run elsewhere.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::human_interaction::*;

type Result<T> = std::result::Result<T, HumanInteractionError>;

/// Native Host ownership, deliberately not serializable/deserializable as model or IPC input.
#[derive(Debug, Clone)]
pub struct HostHumanInteractionOwner {
    pub agent_id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub tool_call_id: String,
}

/// Private checkpoint storage foundation. No public snapshot includes resume authority.
#[derive(Clone, PartialEq)]
pub(crate) struct HumanInteractionSuspension {
    pub request_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub tool_call_id: String,
    pub checkpoint: serde_json::Value,
}

pub(super) fn unavailable<T>(_: T) -> HumanInteractionError {
    HumanInteractionError::new(
        "storage_unavailable",
        "Human interaction storage is unavailable.",
    )
}
fn conflict() -> HumanInteractionError {
    HumanInteractionError::new("conflict", "The human interaction state has changed.")
}
fn not_found() -> HumanInteractionError {
    HumanInteractionError::new("not_found", "The human interaction request was not found.")
}
fn safe_revision(revision: u64) -> Result<()> {
    if revision > HUMAN_INTERACTION_MAX_SAFE_INTEGER {
        return Err(HumanInteractionError::invalid());
    }
    Ok(())
}
fn valid_time(now: i64) -> Result<()> {
    if now < 0 || now as u64 > HUMAN_INTERACTION_MAX_SAFE_INTEGER {
        return Err(HumanInteractionError::invalid());
    }
    Ok(())
}
fn json<T: Serialize + ?Sized>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(unavailable)
}
fn parse<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(unavailable)
}
fn enum_name<T: Serialize>(value: T) -> Result<String> {
    serde_json::from_value(serde_json::to_value(value).map_err(unavailable)?).map_err(unavailable)
}
fn parse_enum<T: serde::de::DeserializeOwned>(value: String) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(value)).map_err(unavailable)
}

pub fn load_settings(connection: &Connection) -> Result<HumanInteractionSettings> {
    connection.query_row(
        "SELECT enabled, revision, updated_at FROM human_interaction_settings WHERE singleton=1",
        [], |row| Ok(HumanInteractionSettings { enabled: row.get(0)?, revision: row.get(1)?, updated_at: row.get(2)? }),
    ).map_err(unavailable)
}

pub fn update_settings(
    connection: &mut Connection,
    input: &HumanInteractionSettingsUpdate,
    now: i64,
) -> Result<HumanInteractionSettings> {
    safe_revision(input.expected_revision)?;
    valid_time(now)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let old = load_settings(&tx)?;
    if old.revision != input.expected_revision || old.revision == HUMAN_INTERACTION_MAX_SAFE_INTEGER
    {
        return Err(HumanInteractionError::new(
            "revision_conflict",
            "The human interaction settings have changed.",
        ));
    }
    tx.execute("UPDATE human_interaction_settings SET enabled=?1, revision=revision+1, updated_at=?2 WHERE singleton=1 AND revision=?3",
        params![input.enabled, now.max(old.updated_at), input.expected_revision]).map_err(unavailable)?;
    let result = load_settings(&tx)?;
    tx.commit().map_err(unavailable)?;
    Ok(result)
}

fn validate_owner(connection: &Connection, owner: &HostHumanInteractionOwner) -> Result<()> {
    for id in [
        &owner.agent_id,
        &owner.conversation_id,
        &owner.run_id,
        &owner.assistant_message_id,
        &owner.tool_call_id,
    ] {
        validate_human_interaction_id(id)?;
    }
    let authorized: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_nodes n
         JOIN conversation_turn_traces t ON t.conversation_id=n.conversation_id
         JOIN messages m ON m.id=t.assistant_message_id AND m.conversation_id=n.conversation_id
         WHERE n.agent_id=?1 AND n.conversation_id=?2 AND n.parent_agent_id IS NULL
           AND n.root_agent_id=n.agent_id AND n.root_conversation_id=n.conversation_id AND n.lifecycle='active'
           AND t.run_id=?3 AND t.assistant_message_id=?4 AND t.terminal_status='in_progress' AND m.role='assistant'
           AND NOT EXISTS(SELECT 1 FROM automation_runs a WHERE a.agent_run_id=t.run_id OR a.assistant_message_id=t.assistant_message_id)
           AND NOT EXISTS(SELECT 1 FROM agent_tree_run_stops s WHERE s.run_id=t.run_id))",
        params![owner.agent_id, owner.conversation_id, owner.run_id, owner.assistant_message_id], |row| row.get(0),
    ).map_err(unavailable)?;
    if !authorized {
        return Err(HumanInteractionError::new(
            "invalid_owner",
            "Only an active interactive root run can create human questions.",
        ));
    }
    Ok(())
}

pub fn create_request(
    connection: &mut Connection,
    owner: &HostHumanInteractionOwner,
    mode: HumanInteractionMode,
    input: &HumanInteractionToolInput,
    now: i64,
) -> Result<HumanInteractionRequestSnapshot> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let result = create_request_in_transaction(&tx, owner, mode, input, now)?;
    tx.commit().map_err(unavailable)?;
    Ok(result)
}

fn create_request_in_transaction(
    tx: &Connection,
    owner: &HostHumanInteractionOwner,
    mode: HumanInteractionMode,
    input: &HumanInteractionToolInput,
    now: i64,
) -> Result<HumanInteractionRequestSnapshot> {
    validate_human_interaction_tool_input(input)?;
    valid_time(now)?;
    let questions: Vec<_> = input
        .questions
        .iter()
        .map(|q| HumanInteractionQuestion {
            id: Uuid::new_v4().to_string(),
            title: q.title.clone(),
            options: q.options.as_ref().map(|options| {
                options
                    .iter()
                    .map(|label| HumanInteractionOption {
                        id: Uuid::new_v4().to_string(),
                        label: label.clone(),
                    })
                    .collect()
            }),
        })
        .collect();
    // Generated IDs consume transport bytes too. Every admitted batch must allow at least a
    // complete skipped response without introducing a product question-count limit.
    let minimum_answers: Vec<_> = questions
        .iter()
        .map(|q| HumanInteractionAnswer::Skipped {
            question_id: q.id.clone(),
        })
        .collect();
    validate_human_interaction_answers(&questions, &minimum_answers)?;
    let questions_json = json(&questions)?;
    if questions_json.len() > 1_048_576 {
        return Err(HumanInteractionError::invalid());
    }
    validate_owner(tx, owner)?;
    let settings = load_settings(tx)?;
    if !settings.enabled {
        return Err(HumanInteractionError::new(
            "disabled",
            "Human questions are disabled.",
        ));
    }
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM human_interaction_requests WHERE (run_id=?1 AND tool_call_id=?2) OR (?3='sync' AND conversation_id=?4 AND mode='sync' AND status='open'))",
        params![owner.run_id, owner.tool_call_id, enum_name(mode)?, owner.conversation_id], |row| row.get(0),
    ).map_err(unavailable)?;
    if exists {
        return Err(conflict());
    }
    let request_id = Uuid::new_v4().to_string();
    tx.execute("INSERT INTO human_interaction_requests(request_id,conversation_id,agent_id,run_id,assistant_message_id,tool_call_id,mode,status,revision,policy_revision,questions_json,created_at,updated_at)
        VALUES(?1,?2,?3,?4,?5,?6,?7,'open',0,?8,?9,?10,?10)",
        params![request_id, owner.conversation_id, owner.agent_id, owner.run_id, owner.assistant_message_id, owner.tool_call_id, enum_name(mode)?, settings.revision, questions_json, now]).map_err(unavailable)?;
    load_request(tx, &owner.conversation_id, &request_id)
}

fn load_request(
    connection: &Connection,
    conversation_id: &str,
    request_id: &str,
) -> Result<HumanInteractionRequestSnapshot> {
    let raw = connection.query_row("SELECT request_id,conversation_id,run_id,assistant_message_id,tool_call_id,mode,status,revision,policy_revision,questions_json,created_at,updated_at,sequence
        FROM human_interaction_requests WHERE conversation_id=?1 AND request_id=?2", params![conversation_id, request_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, String>(5)?, row.get::<_, String>(6)?, row.get::<_, u64>(7)?, row.get::<_, u64>(8)?, row.get::<_, String>(9)?, row.get::<_, i64>(10)?, row.get::<_, i64>(11)?, row.get::<_, u64>(12)?))
    }).optional().map_err(unavailable)?.ok_or_else(not_found)?;
    let response_raw = connection.query_row("SELECT response_id,submission_id,kind,answers_json,created_at FROM human_interaction_responses WHERE request_id=?1", [request_id], |row| {
        Ok((row.get::<_, String>(0)?,row.get::<_, String>(1)?,row.get::<_, String>(2)?,row.get::<_, String>(3)?,row.get::<_, i64>(4)?))
    }).optional().map_err(unavailable)?;
    let response = response_raw
        .map(|r| {
            Ok(HumanInteractionResponse {
                response_id: r.0,
                request_id: request_id.to_owned(),
                submission_id: r.1,
                kind: parse_enum(r.2)?,
                answers: parse(&r.3)?,
                created_at: r.4,
            })
        })
        .transpose()?;
    let delivery = if let Some(response) = &response {
        connection.query_row("SELECT status,revision,target_run_id,user_message_id,error_code FROM human_interaction_deliveries WHERE response_id=?1", [&response.response_id], |row| {
            Ok((row.get::<_, String>(0)?,row.get::<_, u64>(1)?,row.get::<_, Option<String>>(2)?,row.get::<_, Option<String>>(3)?,row.get::<_, Option<String>>(4)?))
        }).optional().map_err(unavailable)?.map(|d| Ok(HumanInteractionDelivery { response_id: response.response_id.clone(), status: parse_enum(d.0)?, revision: d.1, target_run_id: d.2, user_message_id: d.3, error_code: d.4 })).transpose()?
    } else {
        None
    };
    let snapshot = HumanInteractionRequestSnapshot {
        schema_version: HUMAN_INTERACTION_SCHEMA_VERSION,
        sequence: raw.12,
        request_id: raw.0,
        conversation_id: raw.1,
        run_id: raw.2,
        assistant_message_id: raw.3,
        tool_call_id: raw.4,
        mode: parse_enum(raw.5)?,
        status: parse_enum(raw.6)?,
        revision: raw.7,
        policy_revision: raw.8,
        questions: parse(&raw.9)?,
        response,
        delivery,
        created_at: raw.10,
        updated_at: raw.11,
    };
    validate_snapshot(&snapshot).map_err(unavailable)?;
    Ok(snapshot)
}

fn validate_snapshot(snapshot: &HumanInteractionRequestSnapshot) -> Result<()> {
    for id in [
        &snapshot.request_id,
        &snapshot.conversation_id,
        &snapshot.run_id,
        &snapshot.assistant_message_id,
        &snapshot.tool_call_id,
    ] {
        validate_human_interaction_id(id)?;
    }
    safe_revision(snapshot.revision)?;
    safe_revision(snapshot.policy_revision)?;
    safe_revision(snapshot.sequence)?;
    if snapshot.sequence == 0 {
        return Err(HumanInteractionError::invalid());
    }
    valid_time(snapshot.created_at)?;
    valid_time(snapshot.updated_at)?;
    if snapshot.updated_at < snapshot.created_at {
        return Err(HumanInteractionError::invalid());
    }
    let mut ids = std::collections::BTreeSet::new();
    for question in &snapshot.questions {
        validate_human_interaction_id(&question.id)?;
        if !ids.insert(&question.id) {
            return Err(HumanInteractionError::invalid());
        }
        if let Some(options) = &question.options {
            for option in options {
                validate_human_interaction_id(&option.id)?;
                if !ids.insert(&option.id) {
                    return Err(HumanInteractionError::invalid());
                }
            }
        }
    }
    validate_human_interaction_tool_input(&HumanInteractionToolInput {
        questions: snapshot
            .questions
            .iter()
            .map(|q| HumanInteractionQuestionInput {
                title: q.title.clone(),
                options: q
                    .options
                    .as_ref()
                    .map(|options| options.iter().map(|option| option.label.clone()).collect()),
            })
            .collect(),
    })?;
    match (&snapshot.status, &snapshot.response, &snapshot.delivery) {
        (
            HumanInteractionRequestStatus::Open | HumanInteractionRequestStatus::Cancelled,
            None,
            None,
        ) => {}
        (HumanInteractionRequestStatus::Submitted, Some(response), Some(delivery))
            if response.kind == HumanInteractionResponseKind::Submitted =>
        {
            if validate_human_interaction_answers(&snapshot.questions, &response.answers)?
                != response.answers
                || delivery.response_id != response.response_id
            {
                return Err(HumanInteractionError::invalid());
            }
            safe_revision(delivery.revision)?;
            if delivery.status == HumanInteractionDeliveryStatus::Pending
                && (delivery.target_run_id.is_some()
                    || delivery.user_message_id.is_some()
                    || delivery.error_code.is_some())
            {
                return Err(HumanInteractionError::invalid());
            }
        }
        (HumanInteractionRequestStatus::Ignored, Some(response), None)
            if snapshot.mode == HumanInteractionMode::Async
                && response.kind == HumanInteractionResponseKind::Ignored
                && response.answers.is_empty() => {}
        _ => return Err(HumanInteractionError::invalid()),
    }
    if let Some(response) = &snapshot.response {
        validate_human_interaction_id(&response.response_id)?;
        validate_human_interaction_id(&response.submission_id)?;
        valid_time(response.created_at)?;
        if response.request_id != snapshot.request_id
            || snapshot.revision == 0
            || response.created_at < snapshot.created_at
            || response.created_at > snapshot.updated_at
        {
            return Err(HumanInteractionError::invalid());
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListCursor {
    version: u8,
    conversation_id: String,
    sequence: u64,
}

pub fn list_requests(
    connection: &mut Connection,
    input: &HumanInteractionListInput,
) -> Result<HumanInteractionListOutput> {
    validate_human_interaction_id(&input.conversation_id)?;
    if !(1..=100).contains(&input.limit) {
        return Err(HumanInteractionError::invalid());
    }
    let cursor: Option<ListCursor> = input
        .cursor
        .as_ref()
        .map(|encoded| {
            if encoded.is_empty() || encoded.len() > 2048 {
                return Err(HumanInteractionError::invalid());
            }
            let bytes = URL_SAFE_NO_PAD
                .decode(encoded)
                .map_err(|_| HumanInteractionError::invalid())?;
            let cursor: ListCursor =
                serde_json::from_slice(&bytes).map_err(|_| HumanInteractionError::invalid())?;
            if cursor.version != 1 || cursor.conversation_id != input.conversation_id {
                return Err(HumanInteractionError::invalid());
            }
            safe_revision(cursor.sequence)?;
            if cursor.sequence == 0 {
                return Err(HumanInteractionError::invalid());
            }
            Ok(cursor)
        })
        .transpose()?;
    let tx = connection.transaction().map_err(unavailable)?;
    let ids: Vec<String> = {
        let mut statement = tx
            .prepare(
                "SELECT request_id FROM human_interaction_requests WHERE conversation_id=?1
            AND (?2 IS NULL OR sequence<?2)
            ORDER BY sequence DESC LIMIT ?3",
            )
            .map_err(unavailable)?;
        let rows = statement
            .query_map(
                params![
                    input.conversation_id,
                    cursor.as_ref().map(|c| c.sequence),
                    input.limit + 1
                ],
                |row| row.get(0),
            )
            .map_err(unavailable)?;
        rows.collect::<rusqlite::Result<_>>().map_err(unavailable)?
    };
    let has_more = ids.len() > input.limit as usize;
    let items: Vec<_> = ids
        .iter()
        .take(input.limit as usize)
        .map(|id| load_request(&tx, &input.conversation_id, id))
        .collect::<Result<_>>()?;
    let next_cursor = if has_more {
        let last = items.last().ok_or_else(|| unavailable(()))?;
        Some(URL_SAFE_NO_PAD.encode(json(&ListCursor {
            version: 1,
            conversation_id: input.conversation_id.clone(),
            sequence: last.sequence,
        })?))
    } else {
        None
    };
    tx.commit().map_err(unavailable)?;
    Ok(HumanInteractionListOutput { items, next_cursor })
}

pub fn submit(
    connection: &mut Connection,
    input: &HumanInteractionSubmitInput,
    now: i64,
) -> Result<HumanInteractionRequestSnapshot> {
    settle(
        connection,
        &input.conversation_id,
        &input.request_id,
        input.expected_revision,
        &input.submission_id,
        Some(&input.answers),
        now,
    )
}

pub fn ignore(
    connection: &mut Connection,
    input: &HumanInteractionIgnoreInput,
    now: i64,
) -> Result<HumanInteractionRequestSnapshot> {
    settle(
        connection,
        &input.conversation_id,
        &input.request_id,
        input.expected_revision,
        &input.submission_id,
        None,
        now,
    )
}

fn settle(
    connection: &mut Connection,
    conversation_id: &str,
    request_id: &str,
    expected_revision: u64,
    submission_id: &str,
    answers: Option<&[HumanInteractionAnswer]>,
    now: i64,
) -> Result<HumanInteractionRequestSnapshot> {
    for id in [conversation_id, request_id, submission_id] {
        validate_human_interaction_id(id)?;
    }
    safe_revision(expected_revision)?;
    valid_time(now)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let request = load_request(&tx, conversation_id, request_id)?;
    let kind = if answers.is_some() {
        HumanInteractionResponseKind::Submitted
    } else {
        HumanInteractionResponseKind::Ignored
    };
    if kind == HumanInteractionResponseKind::Ignored && request.mode != HumanInteractionMode::Async
    {
        return Err(HumanInteractionError::new(
            "invalid_state",
            "Only asynchronous questions can be ignored.",
        ));
    }
    let ordered = answers
        .map(|answers| validate_human_interaction_answers(&request.questions, answers))
        .transpose()?
        .unwrap_or_default();
    if let Some(response) = &request.response {
        let base_revision: u64 = tx
            .query_row(
                "SELECT base_revision FROM human_interaction_responses WHERE request_id=?1",
                [request_id],
                |row| row.get(0),
            )
            .map_err(unavailable)?;
        if response.submission_id == submission_id
            && response.kind == kind
            && response.answers == ordered
            && base_revision == expected_revision
        {
            tx.commit().map_err(unavailable)?;
            return Ok(request);
        }
        return Err(conflict());
    }
    if request.mode == HumanInteractionMode::Sync
        && tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_tree_run_stops WHERE run_id=?1)",
                [&request.run_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(unavailable)?
    {
        return Err(conflict());
    }
    if request.status != HumanInteractionRequestStatus::Open
        || request.revision != expected_revision
        || request.revision == HUMAN_INTERACTION_MAX_SAFE_INTEGER
    {
        return Err(conflict());
    }
    let now = now.max(request.updated_at);
    let status = if kind == HumanInteractionResponseKind::Submitted {
        "submitted"
    } else {
        "ignored"
    };
    let changed = tx.execute("UPDATE human_interaction_requests SET status=?1,revision=revision+1,updated_at=?2 WHERE request_id=?3 AND status='open' AND revision=?4", params![status,now,request_id,expected_revision]).map_err(unavailable)?;
    if changed != 1 {
        return Err(conflict());
    }
    let response_id = Uuid::new_v4().to_string();
    tx.execute("INSERT INTO human_interaction_responses(response_id,request_id,submission_id,base_revision,kind,answers_json,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![response_id,request_id,submission_id,expected_revision,enum_name(kind)?,json(&ordered)?,now]).map_err(unavailable)?;
    if kind == HumanInteractionResponseKind::Submitted {
        tx.execute("INSERT INTO human_interaction_deliveries(response_id,status,revision,target_run_id,user_message_id,error_code) VALUES(?1,'pending',0,NULL,NULL,NULL)", [&response_id]).map_err(unavailable)?;
    }
    let result = load_request(&tx, conversation_id, request_id)?;
    tx.commit().map_err(unavailable)?;
    Ok(result)
}

/// Stores opaque future-runtime material only. This does not pause a worker, certify the
/// checkpoint as executable, publish waiting state, or grant resume authority to the client.
#[allow(dead_code)]
pub(crate) fn save_suspension(
    connection: &mut Connection,
    input: &HumanInteractionSuspension,
    now: i64,
) -> Result<()> {
    valid_time(now)?;
    for id in [
        &input.request_id,
        &input.run_id,
        &input.assistant_message_id,
        &input.tool_call_id,
    ] {
        validate_human_interaction_id(id)?;
    }
    let checkpoint = json(&input.checkpoint)?;
    if !input.checkpoint.is_object() || checkpoint.len() > 1_048_576 {
        return Err(HumanInteractionError::invalid());
    }
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let binding: Option<(String, String)> = tx.query_row("SELECT conversation_id,agent_id FROM human_interaction_requests WHERE request_id=?1 AND mode='sync' AND status='open' AND run_id=?2 AND assistant_message_id=?3 AND tool_call_id=?4",
        params![input.request_id,input.run_id,input.assistant_message_id,input.tool_call_id], |row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(unavailable)?;
    let (conversation_id, agent_id) = binding.ok_or_else(conflict)?;
    validate_owner(
        &tx,
        &HostHumanInteractionOwner {
            agent_id,
            conversation_id,
            run_id: input.run_id.clone(),
            assistant_message_id: input.assistant_message_id.clone(),
            tool_call_id: input.tool_call_id.clone(),
        },
    )?;
    if let Some(old) = load_suspension(&tx, &input.request_id)? {
        if old != *input {
            return Err(conflict());
        }
    } else {
        tx.execute("INSERT INTO human_interaction_suspensions(request_id,run_id,assistant_message_id,tool_call_id,checkpoint_json,status,revision,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,'waiting',0,?6,?6)",
            params![input.request_id,input.run_id,input.assistant_message_id,input.tool_call_id,checkpoint,now]).map_err(unavailable)?;
    }
    tx.commit().map_err(unavailable)?;
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn load_suspension(
    connection: &Connection,
    request_id: &str,
) -> Result<Option<HumanInteractionSuspension>> {
    validate_human_interaction_id(request_id)?;
    connection.query_row("SELECT run_id,assistant_message_id,tool_call_id,checkpoint_json FROM human_interaction_suspensions WHERE request_id=?1", [request_id], |row| {
        Ok((row.get::<_, String>(0)?,row.get::<_, String>(1)?,row.get::<_, String>(2)?,row.get::<_, String>(3)?))
    }).optional().map_err(unavailable)?.map(|row| Ok(HumanInteractionSuspension { request_id: request_id.to_owned(), run_id: row.0, assistant_message_id: row.1, tool_call_id: row.2, checkpoint: parse(&row.3)? })).transpose()
}

#[cfg(test)]
mod tests;

mod sync;
pub use sync::*;
