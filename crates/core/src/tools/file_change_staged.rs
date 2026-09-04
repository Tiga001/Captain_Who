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
    observation_id: Option<String>,
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
    observation_id: Option<String>,
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
    let public_observation_id = observation_id.as_deref();
    let observation = match operation {
        FileChangeOperation::Create => {
            if public_observation_id.is_some() {
                return Err(file_change_agent_error(FileChangeError::new(
                    FileChangeErrorCode::IllegalFieldCombination,
                )));
            }
            super::apply_patch::capture_missing_base_observation(context, &target)
                .map_err(|error| file_change_agent_error_for_path(error, &file_path))?
        }
        FileChangeOperation::Update => {
            let observation_id = public_observation_id.ok_or_else(|| {
                file_change_agent_error_for_path(
                    FileChangeError::new(FileChangeErrorCode::ObservationRequired),
                    &file_path,
                )
            })?;
            context
                .file_observations()
                .validate(
                    observation_id,
                    context.conversation_id()?,
                    context.run_id()?,
                    target.absolute_path(),
                )
                .map_err(|error| file_change_agent_error_for_path(error, &file_path))?
        }
        FileChangeOperation::Delete => unreachable!("validated staged begin rejects delete"),
    };
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
        observation_id: observation.id().to_string(),
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
    if let Some(observation_id) = public_observation_id {
        context
            .file_observations()
            .claim(
                observation_id,
                context.conversation_id()?,
                context.run_id()?,
                target.absolute_path(),
            )
            .map_err(|error| file_change_agent_error_for_path(error, &file_path))?;
    }
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
mod tests;
