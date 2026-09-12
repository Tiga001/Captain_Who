use super::*;

pub(super) fn non_model_file_change_result_projection(result: &AgentToolResult) -> AgentToolResult {
    without_successor_observation_projection(&file_change_staged::public_result_projection(result))
}

pub(crate) fn without_successor_observation_projection(
    result: &AgentToolResult,
) -> AgentToolResult {
    let mut projected = result.clone();
    if let Some(output) = projected.result.as_mut().and_then(Value::as_object_mut) {
        for key in [
            "observationId",
            "fileChangeTarget",
            "observationRefreshRequired",
            "continueWith",
        ] {
            output.remove(key);
        }
    }
    projected
}

/// Adds a model-only Observation for the state authoritatively produced by a successful
/// `apply_patch` call.
///
/// The Host owns the commit and its durable receipt, while the live Runtime owns the run-scoped
/// Observation registry. The Runtime therefore reopens the exact target after settlement,
/// verifies the committed Revision (or verified absence for delete), and only then issues the
/// first opaque Observation for create or renews the consumed id for update/delete. The
/// authoritative result is never changed, so Renderer, audit, and
/// ordinary durable result projections cannot accidentally treat this convenience capability as
/// commit proof. The same model-only envelope is retained in the private run checkpoint so a
/// queued follow-up call can prove where its token came from.
///
/// If an external actor races the post-commit verification, the file mutation remains successful
/// but no Observation is issued. The model receives an explicit `read_file` continuation instead
/// of a guessed or stale token.
pub(crate) fn attach_successor_observation_to_model_result(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
    authoritative_result: &AgentToolResult,
    model_result: &mut AgentToolResult,
) -> bool {
    attach_successor_observation_to_model_result_inner(
        context,
        call,
        authoritative_result,
        model_result,
        None,
        None,
    )
}

pub(crate) fn attach_successor_observation_to_model_result_with_proposal(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
    authoritative_result: &AgentToolResult,
    model_result: &mut AgentToolResult,
    proposal: &AgentFileChangeProposal,
) -> bool {
    attach_successor_observation_to_model_result_inner(
        context,
        call,
        authoritative_result,
        model_result,
        Some(proposal),
        None,
    )
}

pub(crate) fn attach_successor_observation_to_model_result_with_predecessor(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
    authoritative_result: &AgentToolResult,
    model_result: &mut AgentToolResult,
    predecessor_observation_id: &str,
) -> bool {
    attach_successor_observation_to_model_result_inner(
        context,
        call,
        authoritative_result,
        model_result,
        None,
        Some(predecessor_observation_id),
    )
}

fn attach_successor_observation_to_model_result_inner(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
    authoritative_result: &AgentToolResult,
    model_result: &mut AgentToolResult,
    proposal: Option<&AgentFileChangeProposal>,
    predecessor_observation_id: Option<&str>,
) -> bool {
    let Some(terminal) = successful_file_change_result(call, authoritative_result, proposal) else {
        return false;
    };
    let predecessor_observation_id = predecessor_observation_id
        .or_else(|| successor_predecessor_observation_id(call, &terminal, proposal));
    let observation =
        issue_successor_observation(context, call, &terminal, predecessor_observation_id);
    let Some(output) = model_result.result.as_mut().and_then(Value::as_object_mut) else {
        return false;
    };
    match observation {
        Ok((observation_id, state)) => {
            output.insert(
                "observationId".to_string(),
                Value::String(observation_id.clone()),
            );
            output.insert(
                "fileChangeTarget".to_string(),
                json!({
                    "filePath": terminal.file_path,
                    "observationId": observation_id,
                    "state": state,
                }),
            );
            true
        }
        Err(_) => {
            output.insert("observationRefreshRequired".to_string(), Value::Bool(true));
            output.insert(
                "continueWith".to_string(),
                json!({
                    "tool": "read_file",
                    "args": { "path": terminal.file_path },
                }),
            );
            false
        }
    }
}

pub(crate) fn copy_successor_observation_projection(
    source: &AgentToolResult,
    target: &mut AgentToolResult,
) {
    let (Some(source), Some(target)) = (
        source.result.as_ref().and_then(Value::as_object),
        target.result.as_mut().and_then(Value::as_object_mut),
    ) else {
        return;
    };
    for key in [
        "observationId",
        "fileChangeTarget",
        "observationRefreshRequired",
        "continueWith",
    ] {
        if let Some(value) = source.get(key) {
            target.insert(key.to_string(), value.clone());
        }
    }
}

fn successful_file_change_result(
    call: &AgentToolCall,
    result: &AgentToolResult,
    proposal: Option<&AgentFileChangeProposal>,
) -> Option<AgentFileChangeResult> {
    if call.tool != "apply_patch"
        || result.tool != "apply_patch"
        || result.call_id != call.id
        || !result.ok
        || !matches!(apply_patch_action(&call.args), Some("apply" | "commit"))
    {
        return None;
    }
    let terminal = serde_json::from_value::<AgentFileChangeResult>(result.result.clone()?).ok()?;
    if !matches!(
        terminal.status,
        AgentFileChangeResultStatus::Applied | AgentFileChangeResultStatus::AlreadyApplied
    ) || !terminal_matches_call(call, &terminal)
        || proposal.is_some_and(|proposal| !terminal_matches_proposal(call, &terminal, proposal))
    {
        return None;
    }
    Some(terminal)
}

fn terminal_matches_call(call: &AgentToolCall, terminal: &AgentFileChangeResult) -> bool {
    let Some(request) = apply_patch_request(&call.args) else {
        return false;
    };
    match request.get("action").and_then(Value::as_str) {
        Some("apply") => {
            request.get("filePath").and_then(Value::as_str) == Some(terminal.file_path.as_str())
                && request.get("operation").and_then(Value::as_str)
                    == Some(operation_label_from_protocol(terminal.operation))
                && terminal.update_strategy.is_none()
        }
        Some("commit") => {
            request.get("transactionId").and_then(Value::as_str)
                == Some(terminal.transaction_id.as_str())
        }
        _ => false,
    }
}

fn terminal_matches_proposal(
    call: &AgentToolCall,
    terminal: &AgentFileChangeResult,
    proposal: &AgentFileChangeProposal,
) -> bool {
    let binding = proposal.execution.as_ref();
    let expected_revision = binding.transaction.target.revision();
    proposal.validate().is_ok()
        && binding.source_tool_name == "apply_patch"
        && binding.source_call_id == call.id
        && crate::file_change::proposal_digest(&call.args)
            .is_ok_and(|digest| digest == binding.source_args_digest)
        && binding.transaction.id == terminal.transaction_id
        && patch_operation(binding.transaction.operation) == terminal.operation
        && proposal.operation == terminal.operation
        && proposal.file_path == terminal.file_path
        && proposal.transaction_id == terminal.transaction_id
        && proposal.update_strategy == terminal.update_strategy
        && expected_revision == terminal.revision.as_deref()
        && match apply_patch_action(&call.args) {
            Some("apply") => binding.staged_transaction_id.is_none(),
            Some("commit") => {
                binding.staged_transaction_id.as_deref() == Some(terminal.transaction_id.as_str())
            }
            _ => false,
        }
}

fn operation_label_from_protocol(operation: AgentFileChangeOperation) -> &'static str {
    match operation {
        AgentFileChangeOperation::Create => "create",
        AgentFileChangeOperation::Update => "update",
        AgentFileChangeOperation::Delete => "delete",
    }
}

fn successor_predecessor_observation_id<'a>(
    call: &'a AgentToolCall,
    terminal: &AgentFileChangeResult,
    proposal: Option<&'a AgentFileChangeProposal>,
) -> Option<&'a str> {
    if terminal.operation == AgentFileChangeOperation::Create {
        return None;
    }
    match apply_patch_action(&call.args) {
        Some("apply") => apply_patch_request(&call.args)?
            .get("observationId")?
            .as_str(),
        Some("commit") => proposal.map(|proposal| proposal.execution.observation_id.as_str()),
        _ => None,
    }
}

fn issue_successor_observation(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
    terminal: &AgentFileChangeResult,
    predecessor_observation_id: Option<&str>,
) -> AgentResult<(String, &'static str)> {
    let target = FileChangePathPolicy::from_workspace(
        context.workspace_context(),
        context.permissions().write == AgentWritePermission::All,
    )
    .resolve(&terminal.file_path)
    .map_err(file_change_agent_error)?;
    let parent = BoundParent::open(&target).map_err(file_change_agent_error)?;
    let bound_context = context.clone().with_tool_call_id(call.id.clone());
    let conversation_id = bound_context.conversation_id()?;
    let run_id = bound_context.run_id()?;
    let observation = match terminal.operation {
        AgentFileChangeOperation::Create | AgentFileChangeOperation::Update => {
            let current = parent
                .read_optional_bounded(
                    target.absolute_path(),
                    crate::file_change::MAX_STAGED_FILE_BYTES,
                )
                .map_err(file_change_agent_error)?
                .ok_or_else(|| {
                    file_change_agent_error(FileChangeError::new(FileChangeErrorCode::FileMissing))
                })?;
            if std::str::from_utf8(&current.bytes).is_err() || current.bytes.contains(&0) {
                return Err(file_change_agent_error(FileChangeError::new(
                    FileChangeErrorCode::UnsupportedFileType,
                )));
            }
            let expected_revision = terminal.revision.as_deref().ok_or_else(|| {
                file_change_agent_error(FileChangeError::new(
                    FileChangeErrorCode::IllegalFieldCombination,
                ))
            })?;
            let actual_revision = content_revision(&current.bytes);
            if actual_revision != expected_revision {
                return Err(file_change_agent_error(FileChangeError::new(
                    FileChangeErrorCode::RevisionConflict,
                )));
            }
            let parent_metadata = parent.parent_metadata().map_err(file_change_agent_error)?;
            let observation = match terminal.operation {
                AgentFileChangeOperation::Create => {
                    bound_context.file_observations().issue_existing(
                        conversation_id,
                        run_id,
                        target.absolute_path(),
                        expected_revision,
                        &current.metadata,
                        &parent_metadata,
                    )
                }
                AgentFileChangeOperation::Update => {
                    let predecessor_observation_id =
                        predecessor_observation_id.ok_or_else(|| {
                            file_change_agent_error(FileChangeError::new(
                                FileChangeErrorCode::ObservationRequired,
                            ))
                        })?;
                    bound_context.file_observations().reissue_existing(
                        predecessor_observation_id,
                        (conversation_id, run_id),
                        target.absolute_path(),
                        expected_revision,
                        &current.metadata,
                        &parent_metadata,
                    )
                }
                AgentFileChangeOperation::Delete => unreachable!("delete has no target bytes"),
            }
            .map_err(file_change_agent_error)?;
            (observation, "existing")
        }
        AgentFileChangeOperation::Delete => {
            if parent
                .read_optional_bounded(target.absolute_path(), 0)
                .map_err(file_change_agent_error)?
                .is_some()
            {
                return Err(file_change_agent_error(FileChangeError::new(
                    FileChangeErrorCode::FileExists,
                )));
            }
            let parent_metadata = parent.parent_metadata().map_err(file_change_agent_error)?;
            let predecessor_observation_id = predecessor_observation_id.ok_or_else(|| {
                file_change_agent_error(FileChangeError::new(
                    FileChangeErrorCode::ObservationRequired,
                ))
            })?;
            let observation = bound_context
                .file_observations()
                .reissue_missing(
                    predecessor_observation_id,
                    conversation_id,
                    run_id,
                    target.absolute_path(),
                    &parent_metadata,
                )
                .map_err(file_change_agent_error)?;
            (observation, "missing")
        }
    };
    Ok((observation.0.id().to_string(), observation.1))
}
