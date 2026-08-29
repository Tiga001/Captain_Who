use super::apply_patch::{
    file_change_agent_error, file_change_agent_error_for_path,
    file_change_agent_error_for_transaction, freeze_staged_observed_base, sanitize_summary,
    state_revision, validate_observation_operation,
};
use super::ToolExecutionContext;
use crate::file_change::{
    allowed_staged_actions, content_digest, is_unsettled_staged_status, proposal_digest,
    FileChangeBase, FileChangeDirectBinding, FileChangeEdit, FileChangeError, FileChangeErrorCode,
    FileChangeMutation, FileChangeMutationReceipt, FileChangeOperation, FileChangeOutcome,
    FileChangePathPolicy, FileChangePlanRequest, FileChangePlanner, FileChangeProposal,
    FileChangeStagedAction, FileChangeStatus, FileChangeTransaction, FileObservationCheckpoint,
    FILE_CHANGE_MUTATION_RECEIPT_SCHEMA_VERSION, FILE_CHANGE_SCHEMA_VERSION,
};
use crate::protocol::{
    AgentApprovalStatus, AgentError, AgentFileChangeProposal, AgentFileChangeUpdateStrategy,
    AgentResult, AgentToolCall, AgentToolResult, AgentWritePermission,
    AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
};
use crate::revision::content_revision;
use crate::storage::file_change_repository::{
    AgentFileChangeProgressSaveOutcome, AGENT_FILE_CHANGE_SCHEMA_VERSION,
};
use crate::storage::models::{
    AgentFileChangeChunkRecord, AgentFileChangeOperationRecord, AgentFileChangeRecord,
};
use crate::storage::now_ms;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};
use uuid::Uuid;

pub(super) const MAX_STAGED_FILE_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_STAGED_CHUNK_BYTES: usize = 1024 * 1024;
const DRAFT_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const RESULT_TAIL_CHARS: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StagedUpdateStrategy {
    Modify,
    Rewrite,
}

impl StagedUpdateStrategy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Modify => "modify",
            Self::Rewrite => "rewrite",
        }
    }
}

pub(super) struct StagedSource<'a> {
    tool_name: &'a str,
    args_digest: String,
}

impl<'a> StagedSource<'a> {
    pub(super) fn new(tool_name: &'a str, args_digest: String) -> Self {
        Self {
            tool_name,
            args_digest,
        }
    }
}

pub(super) fn begin(
    context: &ToolExecutionContext,
    source: StagedSource<'_>,
    operation: FileChangeOperation,
    strategy: Option<StagedUpdateStrategy>,
    file_path: String,
    observation_id: String,
    initial_summary: Option<String>,
) -> AgentResult<Value> {
    begin_with_hook(
        context,
        source,
        operation,
        strategy,
        file_path,
        observation_id,
        initial_summary,
        |_| Ok(()),
    )
}

#[allow(clippy::too_many_arguments)]
fn begin_with_hook(
    context: &ToolExecutionContext,
    source: StagedSource<'_>,
    operation: FileChangeOperation,
    strategy: Option<StagedUpdateStrategy>,
    file_path: String,
    observation_id: String,
    initial_summary: Option<String>,
    after_durable_create: impl FnOnce(&str) -> AgentResult<()>,
) -> AgentResult<Value> {
    require_write(context)?;
    if source.tool_name != "apply_patch"
        || !matches!(
            (operation, strategy),
            (FileChangeOperation::Create, None)
                | (
                    FileChangeOperation::Update,
                    Some(StagedUpdateStrategy::Modify)
                )
                | (
                    FileChangeOperation::Update,
                    Some(StagedUpdateStrategy::Rewrite)
                )
        )
    {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::IllegalFieldCombination,
        )));
    }
    if let Some(existing) = begin_replay_for_source_call(context, &source)? {
        return Ok(existing);
    }
    let target = resolve_target(context, &file_path)?;
    let observation = context
        .file_observations()
        .validate(
            &observation_id,
            context.conversation_id()?,
            context.run_id()?,
            target.absolute_path(),
        )
        .map_err(|error| file_change_agent_error_for_path(error, &file_path))?;
    validate_observation_operation(operation, observation.state())
        .map_err(|error| file_change_agent_error_for_path(error, &file_path))?;
    let frozen = freeze_staged_observed_base(context, &target, &observation, MAX_STAGED_FILE_BYTES)
        .map_err(|error| file_change_agent_error_for_path(error, &file_path))?;
    if frozen
        .content
        .as_deref()
        .is_some_and(|content| content.contains('\0'))
    {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::UnsupportedFileType,
        )));
    }
    let content = match (operation, strategy) {
        (FileChangeOperation::Create, None) | (_, Some(StagedUpdateStrategy::Rewrite)) => {
            String::new()
        }
        (FileChangeOperation::Update, Some(StagedUpdateStrategy::Modify)) => frozen
            .content
            .clone()
            .expect("existing observation freezes existing UTF-8 content"),
        _ => unreachable!("validated staged begin matrix"),
    };
    let now = now_ms();
    let (additions, deletions) = diff_counts(frozen.content.as_deref().unwrap_or(""), &content);
    let checkpoint = observation.checkpoint();
    let transaction = AgentFileChangeRecord {
        schema_version: AGENT_FILE_CHANGE_SCHEMA_VERSION,
        id: format!("file-change-staged-v1:{}", Uuid::new_v4()),
        conversation_id: context.conversation_id()?.to_string(),
        project_id: context.project_id().map(ToString::to_string),
        run_id: context.run_id()?.to_string(),
        source_tool_name: source.tool_name.to_string(),
        source_tool_call_id: context.tool_call_id()?.to_string(),
        source_tool_arguments_digest: source.args_digest.clone(),
        permission_revision: context.file_change_permission_revision().to_string(),
        tool_set_revision: context.file_change_tool_set_revision().to_string(),
        provider_wire_revision: context.file_change_provider_wire_revision().to_string(),
        observation_id: observation_id.clone(),
        observation_json: serde_json::to_string(&checkpoint).map_err(internal_error)?,
        file_path: target.display_path().to_string(),
        operation: operation_label(operation).to_string(),
        strategy: strategy.map(|strategy| strategy.as_str().to_string()),
        status: "drafting".to_string(),
        base_revision: frozen.revision,
        base_content: frozen.content.unwrap_or_default(),
        line_count: line_count(&content),
        byte_count: content.len() as u64,
        content,
        draft_revision: 0,
        next_mutation_index: 0,
        additions,
        deletions,
        mutation_count: 0,
        stats_final: false,
        summary: sanitize_summary(initial_summary),
        final_action_id: None,
        final_action_arguments_digest: None,
        final_permission_revision: None,
        final_tool_set_revision: None,
        final_provider_wire_revision: None,
        created_at: now,
        updated_at: now,
        expires_at: now.saturating_add(DRAFT_TTL_MS),
    };
    validate_record(&transaction)?;
    // Claim only after the complete persistent transaction is known to be semantically valid.
    // A subsequent SQLite failure may require a reread, but can never create a blind writer.
    context
        .file_observations()
        .claim(
            &observation_id,
            context.conversation_id()?,
            context.run_id()?,
            target.absolute_path(),
        )
        .map_err(|error| file_change_agent_error_for_path(error, &file_path))?;
    if let Err(error) = context
        .storage()?
        .create_agent_file_change(transaction.clone())
    {
        if let Some(existing) = begin_replay_for_source_call(context, &source)? {
            return Ok(existing);
        }
        return Err(storage_error(error));
    }
    after_durable_create(&transaction.id)?;
    Ok(transaction_result(&transaction))
}

fn begin_replay_for_source_call(
    context: &ToolExecutionContext,
    source: &StagedSource<'_>,
) -> AgentResult<Option<Value>> {
    let Some(existing) = context
        .storage()?
        .get_agent_file_change_for_source_call(
            context.conversation_id()?,
            context.project_id(),
            context.run_id()?,
            context.tool_call_id()?,
        )
        .map_err(storage_error)?
    else {
        return Ok(None);
    };
    validate_record(&existing)?;
    if existing.source_tool_name != source.tool_name
        || existing.source_tool_arguments_digest != source.args_digest
    {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::ReplayMismatch,
        )));
    }
    Ok(Some(transaction_result(&existing)))
}

pub(super) fn append(
    context: &ToolExecutionContext,
    source_tool_name: &str,
    transaction_id: String,
    index: u64,
    expected_draft_revision: u64,
    content: String,
    source_args_digest: String,
) -> AgentResult<Value> {
    require_write(context)?;
    if content.is_empty() {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::InvalidArguments,
        )));
    }
    reject_text(&content)?;
    if content.len() > MAX_STAGED_CHUNK_BYTES {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::ContentTooLarge,
        )));
    }
    let payload_digest = proposal_digest(&json!({
        "request": {
            "action": "append",
            "transactionId": transaction_id,
            "index": index,
            "expectedDraftRevision": expected_draft_revision,
            "content": content,
        }
    }))
    .map_err(internal_file_error)?;
    let mut transaction = load_owned(context, &transaction_id, source_tool_name)?;
    if let Some(receipt) = replay_receipt(context, &transaction, index, "append", &payload_digest)?
    {
        return Ok(receipt);
    }
    ensure_mutable(&mut transaction, context)?;
    validate_mutation_cursor(&transaction, index, expected_draft_revision)?;
    if transaction.content.len().saturating_add(content.len()) > MAX_STAGED_FILE_BYTES {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::ContentTooLarge,
        )));
    }
    transaction.content.push_str(&content);
    save_mutation(
        context,
        transaction,
        expected_draft_revision,
        PendingMutation {
            index,
            action: "append",
            payload_digest,
            source_args_digest,
            chunk: Some((content_digest(content.as_bytes()), content.len() as u64)),
        },
    )
}

pub(super) fn edit(
    context: &ToolExecutionContext,
    source_tool_name: &str,
    transaction_id: String,
    index: u64,
    expected_draft_revision: u64,
    edits: Vec<FileChangeEdit>,
    source_args_digest: String,
) -> AgentResult<Value> {
    require_write(context)?;
    if edits.is_empty() || edits.len() > 128 {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::InvalidArguments,
        )));
    }
    let payload_digest = proposal_digest(&json!({
        "request": {
            "action": "edit",
            "transactionId": transaction_id,
            "index": index,
            "expectedDraftRevision": expected_draft_revision,
            "edits": edits,
        }
    }))
    .map_err(internal_file_error)?;
    let mut transaction = load_owned(context, &transaction_id, source_tool_name)?;
    if let Some(receipt) = replay_receipt(context, &transaction, index, "edit", &payload_digest)? {
        return Ok(receipt);
    }
    ensure_mutable(&mut transaction, context)?;
    validate_mutation_cursor(&transaction, index, expected_draft_revision)?;
    let revision = content_revision(transaction.content.as_bytes());
    let plan = FileChangePlanner
        .plan(FileChangePlanRequest {
            operation: FileChangeOperation::Update,
            file_path: &transaction.file_path,
            base: FileChangeBase::Existing {
                content: &transaction.content,
                revision: &revision,
            },
            mutation: FileChangeMutation::Edits(edits),
        })
        .map_err(file_change_agent_error)?;
    let updated = plan.target_content.ok_or_else(|| {
        file_change_agent_error(FileChangeError::new(FileChangeErrorCode::Failed))
    })?;
    reject_text(&updated)?;
    transaction.content = updated;
    save_mutation(
        context,
        transaction,
        expected_draft_revision,
        PendingMutation {
            index,
            action: "edit",
            payload_digest,
            source_args_digest,
            chunk: None,
        },
    )
}

pub(super) fn status(
    context: &ToolExecutionContext,
    source_tool_name: &str,
    transaction_id: String,
) -> AgentResult<Value> {
    let mut transaction = load_owned(context, &transaction_id, source_tool_name)?;
    expire_if_needed(context, &mut transaction)?;
    Ok(transaction_result(&transaction))
}

pub(super) fn abort(
    context: &ToolExecutionContext,
    source_tool_name: &str,
    transaction_id: String,
) -> AgentResult<Value> {
    let mut transaction = load_owned(context, &transaction_id, source_tool_name)?;
    expire_if_needed(context, &mut transaction)?;
    ensure_mutable_status(&transaction)?;
    let expected_status = transaction.status.clone();
    transaction.status = "aborted".to_string();
    transaction.stats_final = true;
    transaction.updated_at = now_ms();
    let transitioned = context
        .storage()?
        .transition_agent_file_change(
            &expected_status,
            transaction.draft_revision,
            transaction.next_mutation_index,
            &transaction,
        )
        .map_err(storage_error)?;
    if !transitioned {
        return Err(file_change_agent_error_for_transaction(
            FileChangeError::new(FileChangeErrorCode::DraftRevisionConflict),
            &transaction.id,
        ));
    }
    Ok(transaction_result(&transaction))
}

pub(super) fn commit(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
    source_tool_name: &str,
    transaction_id: String,
    expected_draft_revision: u64,
    summary: Option<String>,
) -> AgentResult<AgentFileChangeProposal> {
    commit_with_hook(
        context,
        call,
        source_tool_name,
        transaction_id,
        expected_draft_revision,
        summary,
        |_, _| Ok(()),
    )
}

fn commit_with_hook(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
    source_tool_name: &str,
    transaction_id: String,
    expected_draft_revision: u64,
    summary: Option<String>,
    after_durable_transition: impl FnOnce(&str, &AgentFileChangeProposal) -> AgentResult<()>,
) -> AgentResult<AgentFileChangeProposal> {
    require_write(context)?;
    let final_action_arguments_digest = proposal_digest(&call.args).map_err(internal_file_error)?;
    let mut stored = load_owned(context, &transaction_id, source_tool_name)?;
    if let Some(replayed) = replay_commit_proposal(
        context,
        call,
        &stored,
        expected_draft_revision,
        &final_action_arguments_digest,
    )? {
        return Ok(replayed);
    }
    ensure_mutable(&mut stored, context)?;
    if stored.draft_revision != expected_draft_revision {
        return Err(file_change_agent_error_for_transaction(
            FileChangeError::new(FileChangeErrorCode::DraftRevisionConflict),
            &transaction_id,
        ));
    }
    stored.summary = sanitize_summary(summary);
    stored.final_action_id = Some(call.id.clone());
    stored.final_action_arguments_digest = Some(final_action_arguments_digest);
    stored.final_permission_revision = Some(context.file_change_permission_revision().to_string());
    stored.final_tool_set_revision = Some(context.file_change_tool_set_revision().to_string());
    stored.final_provider_wire_revision =
        Some(context.file_change_provider_wire_revision().to_string());
    stored.updated_at = now_ms();
    let proposal = build_commit_proposal(context, call, &stored, true)?;
    let expected_status = stored.status.clone();
    stored.status = "waiting_approval".to_string();
    stored.stats_final = true;
    let transitioned = context
        .storage()?
        .transition_agent_file_change(
            &expected_status,
            expected_draft_revision,
            stored.next_mutation_index,
            &stored,
        )
        .map_err(storage_error)?;
    if !transitioned {
        let current = load_owned(context, &transaction_id, source_tool_name)?;
        if let Some(replayed) = replay_commit_proposal(
            context,
            call,
            &current,
            expected_draft_revision,
            stored
                .final_action_arguments_digest
                .as_deref()
                .expect("commit digest was totalized before CAS"),
        )? {
            return Ok(replayed);
        }
        return Err(file_change_agent_error_for_transaction(
            FileChangeError::new(FileChangeErrorCode::DraftRevisionConflict),
            &transaction_id,
        ));
    }
    after_durable_transition(&stored.id, &proposal)?;
    Ok(proposal)
}

fn replay_commit_proposal(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
    stored: &AgentFileChangeRecord,
    expected_draft_revision: u64,
    final_action_arguments_digest: &str,
) -> AgentResult<Option<AgentFileChangeProposal>> {
    if stored.status != "waiting_approval" {
        return Ok(None);
    }
    if stored.draft_revision != expected_draft_revision
        || stored.final_action_id.as_deref() != Some(call.id.as_str())
        || stored.final_action_arguments_digest.as_deref() != Some(final_action_arguments_digest)
    {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::ReplayMismatch,
        )));
    }
    build_commit_proposal(context, call, stored, false).map(Some)
}

fn build_commit_proposal(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
    stored: &AgentFileChangeRecord,
    revalidate_current_identity: bool,
) -> AgentResult<AgentFileChangeProposal> {
    let operation = parse_operation(&stored.operation)?;
    let target = resolve_target(context, &stored.file_path)?;
    let checkpoint: FileObservationCheckpoint = serde_json::from_str(&stored.observation_json)
        .map_err(|_| {
            file_change_agent_error(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
        })?;
    checkpoint
        .validate_frozen_binding(
            context.conversation_id()?,
            context.run_id()?,
            target.absolute_path(),
        )
        .map_err(file_change_agent_error)?;
    if revalidate_current_identity {
        checkpoint
            .revalidate_current_identity(target.absolute_path())
            .map_err(file_change_agent_error)?;
    }
    let base = match operation {
        FileChangeOperation::Create => FileChangeBase::Missing,
        FileChangeOperation::Update => FileChangeBase::Existing {
            content: &stored.base_content,
            revision: stored.base_revision.as_deref().ok_or_else(|| {
                file_change_agent_error(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
            })?,
        },
        FileChangeOperation::Delete => unreachable!("Staged delete is rejected at begin"),
    };
    let plan = FileChangePlanner
        .plan(FileChangePlanRequest {
            operation,
            file_path: target.display_path(),
            base,
            mutation: FileChangeMutation::Complete(stored.content.clone()),
        })
        .map_err(file_change_agent_error)?;
    let transaction = FileChangeTransaction {
        schema_version: FILE_CHANGE_SCHEMA_VERSION,
        id: stored.id.clone(),
        operation,
        file_path: plan.file_path.clone(),
        status: FileChangeStatus::WaitingApproval,
        outcome: FileChangeOutcome::DefinitelyNotExecuted,
        base: plan.base.clone(),
        target: plan.target.clone(),
        proposal_digest: plan.proposal_digest.clone(),
        created_at: stored.created_at.max(0) as u64,
        updated_at: stored.updated_at.max(0) as u64,
    };
    let proposal = FileChangeProposal {
        schema_version: FILE_CHANGE_SCHEMA_VERSION,
        id: call.id.clone(),
        transaction_id: stored.id.clone(),
        operation,
        file_path: plan.file_path.clone(),
        base: plan.base.clone(),
        target: plan.target.clone(),
        diff_digest: plan.diff_digest.clone(),
        proposal_digest: plan.proposal_digest.clone(),
        additions: plan.additions,
        deletions: plan.deletions,
    };
    let execution = FileChangeDirectBinding {
        schema_version: crate::file_change::FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION,
        transaction,
        proposal,
        observation_id: stored.observation_id.clone(),
        observation: checkpoint,
        source_tool_name: "apply_patch".to_string(),
        source_call_id: call.id.clone(),
        source_args_digest: stored
            .final_action_arguments_digest
            .clone()
            .ok_or_else(|| {
                file_change_agent_error(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
            })?,
        trace_args_digest: crate::file_change_support::apply_patch_trace_args_digest(&call.args)
            .map_err(|error| {
                file_change_agent_error(FileChangeError::with_diagnostic(
                    FileChangeErrorCode::Failed,
                    error,
                ))
            })?,
        staged_transaction_id: Some(stored.id.clone()),
        conversation_id: stored.conversation_id.clone(),
        project_id: stored.project_id.clone(),
        run_id: stored.run_id.clone(),
        staged_transaction_revision: Some(stored.draft_revision),
        canonical_target: target.absolute_path().to_string_lossy().into_owned(),
        base_content: plan.base_content.clone(),
        target_content: plan.target_content.clone(),
        delete_journal: None,
        receipt: None,
        permission_revision: stored.final_permission_revision.clone().ok_or_else(|| {
            file_change_agent_error(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
        })?,
        tool_set_revision: stored.final_tool_set_revision.clone().ok_or_else(|| {
            file_change_agent_error(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
        })?,
        provider_wire_revision: stored.final_provider_wire_revision.clone().ok_or_else(|| {
            file_change_agent_error(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
        })?,
    };
    execution.validate().map_err(file_change_agent_error)?;
    Ok(AgentFileChangeProposal {
        schema_version: AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
        id: call.id.clone(),
        transaction_id: stored.id.clone(),
        operation: super::apply_patch::patch_operation(operation),
        update_strategy: match (operation, stored.strategy.as_deref()) {
            (FileChangeOperation::Create, None) => None,
            (FileChangeOperation::Update, Some("modify")) => {
                Some(AgentFileChangeUpdateStrategy::Modify)
            }
            (FileChangeOperation::Update, Some("rewrite")) => {
                Some(AgentFileChangeUpdateStrategy::Rewrite)
            }
            _ => {
                return Err(file_change_agent_error(FileChangeError::new(
                    FileChangeErrorCode::InvalidArguments,
                )))
            }
        },
        file_path: plan.file_path,
        inline_diff: None,
        base_revision: state_revision(&plan.base).map(str::to_string),
        summary: stored.summary.clone(),
        additions: plan.additions,
        deletions: plan.deletions,
        line_count: stored.line_count,
        byte_count: stored.byte_count,
        approval_status: AgentApprovalStatus::Required,
        execution: Box::new(execution),
    })
}

struct PendingMutation<'a> {
    index: u64,
    action: &'a str,
    payload_digest: String,
    source_args_digest: String,
    chunk: Option<(String, u64)>,
}

fn save_mutation(
    context: &ToolExecutionContext,
    mut transaction: AgentFileChangeRecord,
    expected_draft_revision: u64,
    mutation: PendingMutation<'_>,
) -> AgentResult<Value> {
    let PendingMutation {
        index,
        action,
        payload_digest,
        source_args_digest,
        chunk,
    } = mutation;
    let next_revision = expected_draft_revision.checked_add(1).ok_or_else(|| {
        file_change_agent_error(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
    })?;
    transaction.draft_revision = next_revision;
    transaction.next_mutation_index = index.checked_add(1).ok_or_else(|| {
        file_change_agent_error(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
    })?;
    transaction.mutation_count = transaction.next_mutation_index;
    transaction.status = "drafting".to_string();
    transaction.stats_final = false;
    transaction.updated_at = now_ms();
    transaction.expires_at = transaction.updated_at.saturating_add(DRAFT_TTL_MS);
    refresh_metrics(&mut transaction);
    let receipt = mutation_receipt(&transaction, index);
    receipt.validate().map_err(file_change_agent_error)?;
    let receipt_json = serde_json::to_string(&receipt).map_err(internal_error)?;
    let operation = AgentFileChangeOperationRecord {
        transaction_id: transaction.id.clone(),
        mutation_index: index,
        source_tool_call_id: context.tool_call_id()?.to_string(),
        source_tool_arguments_digest: source_args_digest,
        action: action.to_string(),
        payload_digest,
        draft_revision: next_revision,
        receipt_json,
        created_at: transaction.updated_at,
    };
    let chunk = chunk.map(|(digest, byte_count)| AgentFileChangeChunkRecord {
        transaction_id: transaction.id.clone(),
        mutation_index: index,
        content_digest: digest,
        byte_count,
        created_at: transaction.updated_at,
    });
    match context
        .storage()?
        .save_agent_file_change_progress(
            expected_draft_revision,
            &transaction,
            chunk.as_ref(),
            &operation,
        )
        .map_err(storage_error)?
    {
        AgentFileChangeProgressSaveOutcome::Applied => receipt_value(&receipt),
        AgentFileChangeProgressSaveOutcome::Idempotent(existing) => {
            decode_receipt(&existing, &transaction.id, index)
        }
        AgentFileChangeProgressSaveOutcome::ReplayMismatch => Err(file_change_agent_error(
            FileChangeError::new(FileChangeErrorCode::ReplayMismatch),
        )),
        AgentFileChangeProgressSaveOutcome::Conflict => {
            Err(file_change_agent_error_for_transaction(
                FileChangeError::new(FileChangeErrorCode::DraftRevisionConflict),
                &transaction.id,
            ))
        }
    }
}

fn replay_receipt(
    context: &ToolExecutionContext,
    transaction: &AgentFileChangeRecord,
    index: u64,
    action: &str,
    payload_digest: &str,
) -> AgentResult<Option<Value>> {
    let Some(existing) = context
        .storage()?
        .get_agent_file_change_operation(&transaction.id, index)
        .map_err(storage_error)?
    else {
        return Ok(None);
    };
    if existing.action != action || existing.payload_digest != payload_digest {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::ReplayMismatch,
        )));
    }
    decode_receipt(&existing, &transaction.id, index).map(Some)
}

fn decode_receipt(
    operation: &AgentFileChangeOperationRecord,
    transaction_id: &str,
    index: u64,
) -> AgentResult<Value> {
    let receipt: FileChangeMutationReceipt = serde_json::from_str(&operation.receipt_json)
        .map_err(|_| {
            file_change_agent_error(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
        })?;
    receipt.validate().map_err(file_change_agent_error)?;
    if receipt.transaction_id != transaction_id
        || receipt.index != index
        || receipt.draft_revision != operation.draft_revision
    {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::InvalidArguments,
        )));
    }
    receipt_value(&receipt)
}

fn receipt_value(receipt: &FileChangeMutationReceipt) -> AgentResult<Value> {
    serde_json::to_value(receipt).map_err(internal_error)
}

fn mutation_receipt(transaction: &AgentFileChangeRecord, index: u64) -> FileChangeMutationReceipt {
    FileChangeMutationReceipt {
        schema_version: FILE_CHANGE_MUTATION_RECEIPT_SCHEMA_VERSION,
        transaction_id: transaction.id.clone(),
        index,
        draft_revision: transaction.draft_revision,
        next_index: transaction.next_mutation_index,
        byte_count: transaction.byte_count,
        line_count: transaction.line_count,
        allowed_next_actions: mutable_actions(),
        requires_commit_before_response: true,
    }
}

fn load_owned(
    context: &ToolExecutionContext,
    transaction_id: &str,
    source_tool_name: &str,
) -> AgentResult<AgentFileChangeRecord> {
    if source_tool_name != "apply_patch" {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::InvalidArguments,
        )));
    }
    let storage = context.storage()?;
    let Some(transaction) = storage
        .get_agent_file_change(transaction_id)
        .map_err(storage_error)?
    else {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::TransactionNotFound,
        )));
    };
    if transaction.schema_version != AGENT_FILE_CHANGE_SCHEMA_VERSION {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::InvalidArguments,
        )));
    }
    if transaction.conversation_id != context.conversation_id()?
        || transaction.project_id.as_deref() != context.project_id()
        || transaction.run_id != context.run_id()?
        || transaction.source_tool_name != "apply_patch"
    {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::TransactionOwnerMismatch,
        )));
    }
    validate_record(&transaction)?;
    Ok(transaction)
}

fn ensure_mutable(
    transaction: &mut AgentFileChangeRecord,
    context: &ToolExecutionContext,
) -> AgentResult<()> {
    expire_if_needed(context, transaction)?;
    ensure_mutable_status(transaction)
}

fn ensure_mutable_status(transaction: &AgentFileChangeRecord) -> AgentResult<()> {
    if matches!(transaction.status.as_str(), "drafting" | "ready") {
        Ok(())
    } else if transaction.status == "expired" {
        Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::TransactionExpired,
        )))
    } else {
        Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::TransactionSettled,
        )))
    }
}

fn expire_if_needed(
    context: &ToolExecutionContext,
    transaction: &mut AgentFileChangeRecord,
) -> AgentResult<()> {
    if transaction.expires_at > now_ms()
        || !matches!(transaction.status.as_str(), "drafting" | "ready")
    {
        return Ok(());
    }
    let expected_status = transaction.status.clone();
    transaction.status = "expired".to_string();
    transaction.stats_final = true;
    transaction.updated_at = now_ms();
    let transitioned = context
        .storage()?
        .transition_agent_file_change(
            &expected_status,
            transaction.draft_revision,
            transaction.next_mutation_index,
            transaction,
        )
        .map_err(storage_error)?;
    if !transitioned {
        *transaction = load_owned(context, &transaction.id, &transaction.source_tool_name)?;
    }
    Ok(())
}

fn validate_mutation_cursor(
    transaction: &AgentFileChangeRecord,
    index: u64,
    expected_draft_revision: u64,
) -> AgentResult<()> {
    if index != transaction.next_mutation_index {
        return Err(file_change_agent_error_for_transaction(
            FileChangeError::new(FileChangeErrorCode::MutationOutOfOrder),
            &transaction.id,
        ));
    }
    if expected_draft_revision != transaction.draft_revision {
        return Err(file_change_agent_error_for_transaction(
            FileChangeError::new(FileChangeErrorCode::DraftRevisionConflict),
            &transaction.id,
        ));
    }
    Ok(())
}

fn validate_record(transaction: &AgentFileChangeRecord) -> AgentResult<()> {
    let valid_operation = matches!(transaction.operation.as_str(), "create" | "update");
    let valid_strategy = match transaction.operation.as_str() {
        "create" => transaction.strategy.is_none() && transaction.base_revision.is_none(),
        "update" => {
            matches!(transaction.strategy.as_deref(), Some("modify" | "rewrite"))
                && transaction.base_revision.is_some()
        }
        _ => false,
    };
    let valid_final_action = match (
        transaction.final_action_id.as_deref(),
        transaction.final_action_arguments_digest.as_deref(),
        transaction.final_permission_revision.as_deref(),
        transaction.final_tool_set_revision.as_deref(),
        transaction.final_provider_wire_revision.as_deref(),
    ) {
        (None, None, None, None, None) => matches!(
            transaction.status.as_str(),
            "drafting" | "ready" | "aborted" | "expired" | "failed"
        ),
        (
            Some(id),
            Some(digest),
            Some(permission_revision),
            Some(tool_set_revision),
            Some(provider_wire_revision),
        ) => {
            !id.trim().is_empty()
                && !digest.trim().is_empty()
                && !permission_revision.trim().is_empty()
                && !tool_set_revision.trim().is_empty()
                && !provider_wire_revision.trim().is_empty()
                && matches!(
                    transaction.status.as_str(),
                    "waiting_approval"
                        | "applying"
                        | "applied"
                        | "already_applied"
                        | "rejected"
                        | "conflict"
                        | "failed"
                        | "outcome_unknown"
                        | "aborted"
                        | "expired"
                )
        }
        _ => false,
    };
    let valid_stats_final = match transaction.status.as_str() {
        "drafting" | "ready" => !transaction.stats_final,
        "waiting_approval" | "applying" | "applied" | "already_applied" | "rejected"
        | "conflict" | "failed" | "outcome_unknown" | "aborted" | "expired" => {
            transaction.stats_final
        }
        _ => false,
    };
    if transaction.id.trim().is_empty()
        || transaction.conversation_id.trim().is_empty()
        || transaction
            .project_id
            .as_deref()
            .is_some_and(|id| id.trim().is_empty())
        || transaction.run_id.trim().is_empty()
        || transaction.source_tool_name != "apply_patch"
        || transaction.source_tool_call_id.trim().is_empty()
        || transaction.source_tool_arguments_digest.trim().is_empty()
        || transaction.permission_revision.trim().is_empty()
        || transaction.tool_set_revision.trim().is_empty()
        || transaction.provider_wire_revision.trim().is_empty()
        || transaction.observation_id.is_empty()
        || transaction.observation_json.is_empty()
        || !valid_operation
        || !valid_strategy
        || !valid_final_action
        || !valid_stats_final
        || transaction.byte_count != transaction.content.len() as u64
        || transaction.line_count != line_count(&transaction.content)
        || transaction.next_mutation_index != transaction.mutation_count
        || transaction.draft_revision != transaction.mutation_count
        || transaction.byte_count > MAX_STAGED_FILE_BYTES as u64
    {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::InvalidArguments,
        )));
    }
    Ok(())
}

fn refresh_metrics(transaction: &mut AgentFileChangeRecord) {
    let (additions, deletions) = diff_counts(&transaction.base_content, &transaction.content);
    transaction.additions = additions;
    transaction.deletions = deletions;
    transaction.line_count = line_count(&transaction.content);
    transaction.byte_count = transaction.content.len() as u64;
}

fn transaction_result(transaction: &AgentFileChangeRecord) -> Value {
    let (tail, total_chars, tail_start) = tail_preview(&transaction.content, RESULT_TAIL_CHARS);
    json!({
        "transactionId": transaction.id,
        "operation": transaction.operation,
        "strategy": transaction.strategy,
        "filePath": transaction.file_path,
        "status": transaction.status,
        "draftRevision": transaction.draft_revision,
        "nextIndex": transaction.next_mutation_index,
        "byteCount": transaction.byte_count,
        "lineCount": transaction.line_count,
        "mutationCount": transaction.mutation_count,
        "additions": transaction.additions,
        "deletions": transaction.deletions,
        "tail": tail,
        "totalChars": total_chars,
        "tailStart": tail_start,
        "tailTruncated": tail_start > 0,
        "allowedNextActions": allowed_staged_actions(&transaction.status),
        "requiresCommitBeforeResponse": is_unsettled_staged_status(&transaction.status),
    })
}

/// Removes resumability-only text from public event and checkpoint projections.
///
/// The current model may use the bounded tail to continue a long draft, but ordinary Renderer
/// events and public checkpoint DTOs need only stable transaction metadata. The canonical draft
/// remains private in the FileChange store.
pub(super) fn public_result_projection(result: &AgentToolResult) -> AgentToolResult {
    let mut projected = result.clone();
    if let Some(value) = projected.result.as_mut() {
        redact_result_tail(value);
    }
    projected
}

fn redact_result_tail(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if let Some(tail) = object
        .remove("tail")
        .and_then(|tail| tail.as_str().map(str::to_owned))
    {
        object.insert("tailBytes".to_string(), json!(tail.len()));
        object.insert(
            "tailDigest".to_string(),
            json!(content_digest(tail.as_bytes())),
        );
    }
}

fn mutable_actions() -> Vec<FileChangeStagedAction> {
    allowed_staged_actions("drafting")
}

fn resolve_target(
    context: &ToolExecutionContext,
    file_path: &str,
) -> AgentResult<crate::file_change::ResolvedFileChangeTarget> {
    FileChangePathPolicy::new(
        context.workspace_root_optional()?.as_deref(),
        context.permissions().write == AgentWritePermission::All,
    )
    .resolve(file_path)
    .map_err(file_change_agent_error)
}

fn require_write(context: &ToolExecutionContext) -> AgentResult<()> {
    if context.permissions().write == AgentWritePermission::Denied {
        Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::PermissionDenied,
        )))
    } else {
        Ok(())
    }
}

fn reject_text(content: &str) -> AgentResult<()> {
    if content.contains('\0') {
        Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::UnsupportedFileType,
        )))
    } else {
        Ok(())
    }
}

fn operation_label(operation: FileChangeOperation) -> &'static str {
    match operation {
        FileChangeOperation::Create => "create",
        FileChangeOperation::Update => "update",
        FileChangeOperation::Delete => "delete",
    }
}

fn parse_operation(operation: &str) -> AgentResult<FileChangeOperation> {
    match operation {
        "create" => Ok(FileChangeOperation::Create),
        "update" => Ok(FileChangeOperation::Update),
        _ => Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::InvalidArguments,
        ))),
    }
}

fn diff_counts(base: &str, current: &str) -> (u64, u64) {
    TextDiff::from_lines(base, current).iter_all_changes().fold(
        (0_u64, 0_u64),
        |(additions, deletions), change| match change.tag() {
            ChangeTag::Insert => (additions.saturating_add(1), deletions),
            ChangeTag::Delete => (additions, deletions.saturating_add(1)),
            ChangeTag::Equal => (additions, deletions),
        },
    )
}

fn line_count(content: &str) -> u64 {
    if content.is_empty() {
        0
    } else {
        content.lines().count() as u64
    }
}

fn tail_preview(content: &str, limit: usize) -> (String, usize, usize) {
    let total = content.chars().count();
    let start = total.saturating_sub(limit);
    (content.chars().skip(start).collect(), total, start)
}

fn storage_error(error: String) -> AgentError {
    file_change_agent_error(FileChangeError::with_diagnostic(
        FileChangeErrorCode::Failed,
        error,
    ))
}

fn internal_error(error: impl ToString) -> AgentError {
    file_change_agent_error(FileChangeError::with_diagnostic(
        FileChangeErrorCode::Failed,
        error.to_string(),
    ))
}

fn internal_file_error(error: impl ToString) -> AgentError {
    internal_error(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentCommandPermission, AgentPatchPermission, AgentPermissions, AgentReadPermission,
        AgentRunContext, AgentToolCall, AgentWorkspaceContext,
    };
    use crate::storage::models::ChatConversationRecord;
    use crate::storage::service::StorageService;
    use crate::tools::ToolRegistry;
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn context(
        root: &Path,
        storage: Arc<StorageService>,
        conversation_id: &str,
        project_id: Option<&str>,
        run_id: &str,
    ) -> ToolExecutionContext {
        context_with_write(
            root,
            storage,
            conversation_id,
            project_id,
            run_id,
            AgentWritePermission::WorkspaceOnly,
        )
    }

    fn context_with_write(
        root: &Path,
        storage: Arc<StorageService>,
        conversation_id: &str,
        project_id: Option<&str>,
        run_id: &str,
        write: AgentWritePermission,
    ) -> ToolExecutionContext {
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some(conversation_id.to_string()),
            project_id: project_id.map(ToString::to_string),
            workspace: Some(AgentWorkspaceContext {
                project_id: project_id.map(ToString::to_string),
                display_name: Some("workspace".to_string()),
                root_path: Some(root.to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write,
                command: AgentCommandPermission::RequireApproval,
                command_safety: Default::default(),
                patch: AgentPatchPermission::RequireApproval,
                builtin_execution: Default::default(),
            },
        }))
        .with_runtime_services(run_id.to_string(), Some(storage))
        .with_file_change_tool_set_revision("tool-set-staged-test".to_string())
        .with_file_change_provider_wire_revision("provider-wire-staged-test".to_string())
    }

    fn save_conversation(storage: &StorageService, id: &str) {
        storage
            .save_conversation(ChatConversationRecord {
                id: id.to_string(),
                project_id: None,
                model_id: None,
                title: "Test".to_string(),
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
    }

    fn observe(context: &ToolExecutionContext, path: &str, call_id: &str) -> String {
        let result = ToolRegistry::defaults_with_search(None).execute(
            context,
            &AgentToolCall {
                id: call_id.to_string(),
                tool: "read_file".to_string(),
                args: json!({"path": path}),
                approval_status: AgentApprovalStatus::Approved,
                reason: None,
            },
        );
        assert!(result.ok, "{:?}", result.error);
        result.result.unwrap()["observationId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn call_context(context: &ToolExecutionContext, id: &str) -> ToolExecutionContext {
        context.clone().with_tool_call_id(id.to_string())
    }

    #[test]
    fn four_mib_is_the_exact_draft_limit_and_chunk_limit_is_provider_friendly() {
        assert_eq!(MAX_STAGED_FILE_BYTES, 4 * 1024 * 1024);
        assert_eq!(MAX_STAGED_CHUNK_BYTES, 1024 * 1024);
    }

    #[test]
    fn tail_preview_is_utf8_safe() {
        let text = "你".repeat(RESULT_TAIL_CHARS + 3);
        let (tail, total, start) = tail_preview(&text, RESULT_TAIL_CHARS);
        assert_eq!(total, RESULT_TAIL_CHARS + 3);
        assert_eq!(start, 3);
        assert_eq!(tail.chars().count(), RESULT_TAIL_CHARS);
    }

    #[test]
    fn settled_states_do_not_offer_mutation_actions() {
        for status in [
            "waiting_approval",
            "applying",
            "applied",
            "aborted",
            "expired",
        ] {
            assert_eq!(
                allowed_staged_actions(status),
                vec![FileChangeStagedAction::Status]
            );
        }
    }

    #[test]
    fn outcome_unknown_remains_fenced_and_already_applied_is_terminal() {
        assert!(is_unsettled_staged_status("outcome_unknown"));
        assert!(!is_unsettled_staged_status("already_applied"));
    }

    #[test]
    fn public_result_projection_keeps_metadata_without_resumability_text() {
        const CANARY: &str = "PRIVATE_STAGED_RESULT_TAIL_CANARY";
        let raw = AgentToolResult {
            exact_archive_file: None,
            call_id: "call-append".to_string(),
            tool: "apply_patch".to_string(),
            ok: true,
            result: Some(json!({
                "transactionId": "transaction-1",
                "status": "drafting",
                "tail": CANARY,
            })),
            error: None,
        };
        let projected = public_result_projection(&raw);
        let encoded = serde_json::to_string(&projected).unwrap();
        assert!(!encoded.contains(CANARY));
        assert!(encoded.contains("tailBytes"));
        assert!(encoded.contains("tailDigest"));
        assert!(serde_json::to_string(&raw).unwrap().contains(CANARY));
    }

    #[test]
    fn aborted_and_expired_records_allow_only_complete_or_absent_final_identity() {
        let fixture = |status: &str| AgentFileChangeRecord {
            schema_version: AGENT_FILE_CHANGE_SCHEMA_VERSION,
            id: format!("transaction-{status}"),
            conversation_id: "conversation-1".to_string(),
            project_id: None,
            run_id: "run-1".to_string(),
            source_tool_name: "apply_patch".to_string(),
            source_tool_call_id: "call-begin".to_string(),
            source_tool_arguments_digest: content_digest(b"begin"),
            permission_revision: "permission-v1".to_string(),
            tool_set_revision: "tool-set-v1".to_string(),
            provider_wire_revision: "provider-wire-v1".to_string(),
            observation_id: "fobs_current".to_string(),
            observation_json: "{}".to_string(),
            file_path: "report.md".to_string(),
            operation: "create".to_string(),
            strategy: None,
            status: status.to_string(),
            base_revision: None,
            base_content: String::new(),
            content: String::new(),
            draft_revision: 0,
            next_mutation_index: 0,
            additions: 0,
            deletions: 0,
            line_count: 0,
            byte_count: 0,
            mutation_count: 0,
            stats_final: true,
            summary: None,
            final_action_id: None,
            final_action_arguments_digest: None,
            final_permission_revision: None,
            final_tool_set_revision: None,
            final_provider_wire_revision: None,
            created_at: 1,
            updated_at: 2,
            expires_at: 3,
        };

        for status in ["aborted", "expired"] {
            let mut record = fixture(status);
            validate_record(&record).expect("a pre-commit terminal record has no final identity");

            record.final_action_id = Some("call-commit".to_string());
            record.final_action_arguments_digest = Some(content_digest(b"commit"));
            record.final_permission_revision = Some("permission-v2".to_string());
            record.final_tool_set_revision = Some("tool-set-v2".to_string());
            record.final_provider_wire_revision = Some("provider-wire-v2".to_string());
            validate_record(&record)
                .expect("a post-commit terminal record retains its complete frozen identity");

            record.final_provider_wire_revision = None;
            assert!(
                validate_record(&record).is_err(),
                "partial final identity must fail"
            );
        }

        let mut drafting = fixture("drafting");
        drafting.stats_final = false;
        validate_record(&drafting).expect("mutable draft statistics remain provisional");
        drafting.stats_final = true;
        assert!(validate_record(&drafting).is_err());

        let mut settled = fixture("aborted");
        settled.stats_final = false;
        assert!(validate_record(&settled).is_err());
    }

    #[test]
    fn mixed_mutations_are_monotonic_idempotent_and_commit_the_same_file_change_plan() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("workspace");
        fs::create_dir_all(&root).unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        save_conversation(&storage, "conversation-staged");
        let owner_context = context(
            &root,
            storage.clone(),
            "conversation-staged",
            Some("project-staged"),
            "run-staged",
        );
        let observation = observe(&owner_context, "report.md", "read-staged-create");
        let begin_args_digest = content_digest(b"begin-args");
        let begun = begin(
            &call_context(&owner_context, "begin-staged"),
            StagedSource::new("apply_patch", begin_args_digest.clone()),
            FileChangeOperation::Create,
            None,
            "report.md".to_string(),
            observation.clone(),
            None,
        )
        .unwrap();
        let transaction_id = begun["transactionId"].as_str().unwrap().to_string();
        assert_eq!(begun["draftRevision"], 0);
        assert_eq!(begun["nextIndex"], 0);
        assert_eq!(begun["requiresCommitBeforeResponse"], true);
        let begin_replay = begin(
            &call_context(&owner_context, "begin-staged"),
            StagedSource::new("apply_patch", begin_args_digest),
            FileChangeOperation::Create,
            None,
            "report.md".to_string(),
            observation.clone(),
            None,
        )
        .unwrap();
        assert_eq!(begin_replay, begun);
        let begin_mismatch = begin(
            &call_context(&owner_context, "begin-staged"),
            StagedSource::new("apply_patch", content_digest(b"different-begin-args")),
            FileChangeOperation::Create,
            None,
            "different.md".to_string(),
            observation,
            None,
        )
        .unwrap_err();
        assert_eq!(
            begin_mismatch.code(),
            Some("agent.apply_patch.replay_mismatch")
        );

        let append_args_digest = content_digest(b"append-args");
        let appended = append(
            &call_context(&owner_context, "append-staged-1"),
            "apply_patch",
            transaction_id.clone(),
            0,
            0,
            "alpha\n".to_string(),
            append_args_digest.clone(),
        )
        .unwrap();
        assert_eq!(appended["draftRevision"], 1);
        assert_eq!(appended["nextIndex"], 1);

        let replay = append(
            &call_context(&owner_context, "append-staged-retry"),
            "apply_patch",
            transaction_id.clone(),
            0,
            0,
            "alpha\n".to_string(),
            append_args_digest,
        )
        .unwrap();
        assert_eq!(replay, appended);
        let mismatch = append(
            &call_context(&owner_context, "append-staged-mismatch"),
            "apply_patch",
            transaction_id.clone(),
            0,
            0,
            "different\n".to_string(),
            content_digest(b"different-args"),
        )
        .unwrap_err();
        assert_eq!(mismatch.code(), Some("agent.apply_patch.replay_mismatch"));

        let edited = edit(
            &call_context(&owner_context, "edit-staged"),
            "apply_patch",
            transaction_id.clone(),
            1,
            1,
            vec![FileChangeEdit::Append {
                text: "beta\n".to_string(),
            }],
            content_digest(b"edit-args"),
        )
        .unwrap();
        assert_eq!(edited["draftRevision"], 2);
        assert_eq!(edited["byteCount"], 11);

        let out_of_order = edit(
            &call_context(&owner_context, "edit-out-of-order"),
            "apply_patch",
            transaction_id.clone(),
            3,
            2,
            vec![FileChangeEdit::Append { text: "x".into() }],
            content_digest(b"out-of-order"),
        )
        .unwrap_err();
        assert_eq!(
            out_of_order.code(),
            Some("agent.apply_patch.mutation_out_of_order")
        );
        let stale = edit(
            &call_context(&owner_context, "edit-stale"),
            "apply_patch",
            transaction_id.clone(),
            2,
            1,
            vec![FileChangeEdit::Append { text: "x".into() }],
            content_digest(b"stale"),
        )
        .unwrap_err();
        assert_eq!(
            stale.code(),
            Some("agent.apply_patch.draft_revision_conflict")
        );

        let commit_call = AgentToolCall {
            id: "commit-staged".to_string(),
            tool: "apply_patch".to_string(),
            args: json!({
                "request": {
                    "action":"commit",
                    "transactionId":transaction_id,
                    "expectedDraftRevision":2,
                    "summary":"Create report"
                }
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let frozen_proposal = std::cell::RefCell::new(None);
        let injected = commit_with_hook(
            &call_context(&owner_context, "commit-staged"),
            &commit_call,
            "apply_patch",
            transaction_id.clone(),
            2,
            Some("Create report".to_string()),
            |durable_transaction_id, proposal| {
                assert_eq!(durable_transaction_id, transaction_id);
                frozen_proposal.replace(Some(serde_json::to_value(proposal).unwrap()));
                Err(AgentError::new(
                    "injected failure after durable FileChange proposal",
                ))
            },
        )
        .unwrap_err();
        assert!(injected
            .to_string()
            .contains("injected failure after durable FileChange proposal"));
        assert_eq!(
            storage
                .get_agent_file_change(&transaction_id)
                .unwrap()
                .unwrap()
                .status,
            "waiting_approval"
        );

        // Simulate a process boundary after the transaction became durable but before its Tool
        // result/pending receipt was returned. Replay has no in-memory observation and must return
        // the same frozen proposal instead of creating or revalidating a second transaction.
        let recovered_storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        let recovered_context = context(
            &root,
            recovered_storage,
            "conversation-staged",
            Some("project-staged"),
            "run-staged",
        );
        fs::write(root.join("report.md"), "concurrent change\n").unwrap();
        let proposal = commit(
            &call_context(&recovered_context, "commit-staged"),
            &commit_call,
            "apply_patch",
            transaction_id.clone(),
            2,
            Some("Create report".to_string()),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&proposal).unwrap(),
            frozen_proposal
                .into_inner()
                .expect("first frozen proposal was captured after durable transition"),
            "commit replay must return the exact frozen proposal"
        );
        assert_eq!(proposal.transaction_id, transaction_id);
        assert_eq!(proposal.byte_count, 11);
        assert_eq!(
            proposal.execution.target_content.as_deref(),
            Some("alpha\nbeta\n")
        );
        proposal.execution.validate().unwrap();
        let direct_plan = FileChangePlanner
            .plan(FileChangePlanRequest {
                operation: FileChangeOperation::Create,
                file_path: "report.md",
                base: FileChangeBase::Missing,
                mutation: FileChangeMutation::Complete("alpha\nbeta\n".to_string()),
            })
            .unwrap();
        assert_eq!(proposal.execution.transaction.base, direct_plan.base);
        assert_eq!(proposal.execution.transaction.target, direct_plan.target);
        assert_eq!(
            proposal.execution.proposal.diff_digest,
            direct_plan.diff_digest
        );
        assert_eq!(
            proposal.execution.proposal.proposal_digest,
            direct_plan.proposal_digest
        );
        assert_eq!(proposal.additions, direct_plan.additions);
        assert_eq!(proposal.deletions, direct_plan.deletions);
        let mut mismatched_commit = commit_call.clone();
        mismatched_commit.args["summary"] = json!("Different summary");
        let mismatch = commit(
            &call_context(&recovered_context, "commit-staged"),
            &mismatched_commit,
            "apply_patch",
            transaction_id.clone(),
            2,
            Some("Different summary".to_string()),
        )
        .unwrap_err();
        assert_eq!(mismatch.code(), Some("agent.apply_patch.replay_mismatch"));
        let settled = append(
            &call_context(&recovered_context, "append-after-commit"),
            "apply_patch",
            proposal.transaction_id,
            2,
            2,
            "no".to_string(),
            content_digest(b"after"),
        )
        .unwrap_err();
        assert_eq!(
            settled.code(),
            Some("agent.apply_patch.transaction_settled")
        );
    }

    #[test]
    fn staged_transaction_resumes_after_storage_reopen() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("workspace");
        let database = fixture.path().join("storage.sqlite");
        fs::create_dir_all(&root).unwrap();

        let (transaction_id, observation_id) = {
            let storage = Arc::new(StorageService::open(&database).unwrap());
            save_conversation(&storage, "conversation-restart");
            let context = context(
                &root,
                storage.clone(),
                "conversation-restart",
                Some("project-restart"),
                "run-restart",
            );
            let observation = observe(&context, "restart.md", "read-restart-create");
            let injected = begin_with_hook(
                &call_context(&context, "begin-restart"),
                StagedSource::new("apply_patch", content_digest(b"begin-restart")),
                FileChangeOperation::Create,
                None,
                "restart.md".to_string(),
                observation.clone(),
                None,
                |durable_transaction_id| {
                    assert!(durable_transaction_id.starts_with("file-change-staged-v1:"));
                    Err(AgentError::new(
                        "injected failure after durable FileChange begin",
                    ))
                },
            )
            .unwrap_err();
            assert!(injected
                .to_string()
                .contains("injected failure after durable FileChange begin"));
            let begun = storage
                .get_agent_file_change_for_source_call(
                    "conversation-restart",
                    Some("project-restart"),
                    "run-restart",
                    "begin-restart",
                )
                .unwrap()
                .expect("begin transaction was durable before the injected failure");
            (begun.id, observation)
        };

        let storage = Arc::new(StorageService::open(&database).unwrap());
        let context = context(
            &root,
            storage,
            "conversation-restart",
            Some("project-restart"),
            "run-restart",
        );
        let replayed_begin = begin(
            &call_context(&context, "begin-restart"),
            StagedSource::new("apply_patch", content_digest(b"begin-restart")),
            FileChangeOperation::Create,
            None,
            "restart.md".to_string(),
            observation_id,
            None,
        )
        .unwrap();
        assert_eq!(replayed_begin["transactionId"], transaction_id);
        assert_eq!(
            context
                .storage()
                .unwrap()
                .list_agent_file_changes_for_run("run-restart")
                .unwrap()
                .len(),
            1,
            "begin replay after restart must not create a second transaction"
        );
        let restored = status(&context, "apply_patch", transaction_id.clone()).unwrap();
        assert_eq!(restored["draftRevision"], 0);
        assert_eq!(restored["nextIndex"], 0);
        let appended = append(
            &call_context(&context, "append-after-restart"),
            "apply_patch",
            transaction_id.clone(),
            0,
            0,
            "persisted\n".to_string(),
            content_digest(b"append-after-restart"),
        )
        .unwrap();
        assert_eq!(appended["draftRevision"], 1);
        assert_eq!(appended["nextIndex"], 1);
        assert_eq!(
            context
                .storage()
                .unwrap()
                .get_agent_file_change(&transaction_id)
                .unwrap()
                .unwrap()
                .content,
            "persisted\n"
        );
    }

    #[test]
    fn staged_limits_use_bytes_and_reject_nul_or_cross_owner_access() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("workspace");
        fs::create_dir_all(&root).unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        save_conversation(&storage, "conversation-owner");
        save_conversation(&storage, "conversation-other");
        let owner = context(
            &root,
            storage.clone(),
            "conversation-owner",
            None,
            "run-owner",
        );
        let observation = observe(&owner, "large.txt", "read-large");
        let begun = begin(
            &call_context(&owner, "begin-large"),
            StagedSource::new("apply_patch", content_digest(b"begin-large")),
            FileChangeOperation::Create,
            None,
            "large.txt".into(),
            observation,
            None,
        )
        .unwrap();
        let id = begun["transactionId"].as_str().unwrap().to_string();

        let utf8_boundary = format!("{}x", "你".repeat((MAX_STAGED_CHUNK_BYTES - 1) / 3));
        assert_eq!(utf8_boundary.len(), MAX_STAGED_CHUNK_BYTES);
        append(
            &call_context(&owner, "append-1m"),
            "apply_patch",
            id.clone(),
            0,
            0,
            utf8_boundary,
            content_digest(b"one-mib"),
        )
        .unwrap();
        for index in 1..4 {
            append(
                &call_context(&owner, &format!("append-{index}")),
                "apply_patch",
                id.clone(),
                index,
                index,
                "x".repeat(MAX_STAGED_CHUNK_BYTES),
                content_digest(format!("chunk-{index}").as_bytes()),
            )
            .unwrap();
        }
        let over = append(
            &call_context(&owner, "append-over"),
            "apply_patch",
            id.clone(),
            4,
            4,
            "x".to_string(),
            content_digest(b"over"),
        )
        .unwrap_err();
        assert_eq!(over.code(), Some("agent.apply_patch.content_too_large"));
        let nul = append(
            &call_context(&owner, "append-nul"),
            "apply_patch",
            id.clone(),
            4,
            4,
            "\0".to_string(),
            content_digest(b"nul"),
        )
        .unwrap_err();
        assert_eq!(nul.code(), Some("agent.apply_patch.unsupported_file_type"));

        let other_run = context(
            &root,
            storage.clone(),
            "conversation-owner",
            None,
            "run-other",
        );
        let cross_run = status(&other_run, "apply_patch", id.clone()).unwrap_err();
        assert_eq!(
            cross_run.code(),
            Some("agent.apply_patch.transaction_owner_mismatch")
        );
        let other_conversation = context(&root, storage, "conversation-other", None, "run-owner");
        let cross_conversation = status(&other_conversation, "apply_patch", id).unwrap_err();
        assert_eq!(
            cross_conversation.code(),
            Some("agent.apply_patch.transaction_owner_mismatch")
        );
    }

    #[test]
    fn update_strategies_and_abort_status_use_the_same_owned_transaction() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("workspace");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("existing.txt"), "base\n").unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        save_conversation(&storage, "conversation-update");
        let owner = context(
            &root,
            storage.clone(),
            "conversation-update",
            Some("project-update"),
            "run-update",
        );

        let modify_observation = observe(&owner, "existing.txt", "read-modify");
        let modify = begin(
            &call_context(&owner, "begin-modify"),
            StagedSource::new("apply_patch", content_digest(b"begin-modify")),
            FileChangeOperation::Update,
            Some(StagedUpdateStrategy::Modify),
            "existing.txt".into(),
            modify_observation,
            None,
        )
        .unwrap();
        let modify_id = modify["transactionId"].as_str().unwrap().to_string();
        assert_eq!(modify["strategy"], "modify");
        assert_eq!(modify["tail"], "base\n");

        let denied = context_with_write(
            &root,
            storage.clone(),
            "conversation-update",
            Some("project-update"),
            "run-update",
            AgentWritePermission::Denied,
        );
        assert_eq!(
            status(&denied, "apply_patch", modify_id.clone()).unwrap()["status"],
            "drafting"
        );
        let denied_append = append(
            &call_context(&denied, "denied-append"),
            "apply_patch",
            modify_id.clone(),
            0,
            0,
            "x".into(),
            content_digest(b"denied"),
        )
        .unwrap_err();
        assert_eq!(
            denied_append.code(),
            Some("agent.apply_patch.permission_denied")
        );
        let aborted = abort(&denied, "apply_patch", modify_id).unwrap();
        assert_eq!(aborted["status"], "aborted");
        assert_eq!(aborted["requiresCommitBeforeResponse"], false);

        let rewrite_observation = observe(&owner, "existing.txt", "read-rewrite");
        let rewrite = begin(
            &call_context(&owner, "begin-rewrite"),
            StagedSource::new("apply_patch", content_digest(b"begin-rewrite")),
            FileChangeOperation::Update,
            Some(StagedUpdateStrategy::Rewrite),
            "existing.txt".into(),
            rewrite_observation,
            None,
        )
        .unwrap();
        assert_eq!(rewrite["strategy"], "rewrite");
        assert_eq!(rewrite["byteCount"], 0);
        assert_eq!(rewrite["tail"], "");
    }
}
