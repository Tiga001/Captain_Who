fn attach_queued_file_observations(
    queued_calls: &mut [AgentQueuedToolCallCheckpoint],
    registry: &FileObservationRegistry,
    run_id: &str,
    run_context: Option<&AgentRunContext>,
    context_items: &[AgentContextCheckpointItem],
    pending_file_observation: Option<&FileObservationCheckpoint>,
) -> AgentResult<()> {
    let mut observation_ids = BTreeSet::new();
    let mut source_call_ids = BTreeSet::new();
    for queued in queued_calls {
        let Some((observation_id, canonical_target, conversation_id)) =
            queued_apply_patch_observation_request(&queued.call, run_context)?
        else {
            queued.file_observation = None;
            continue;
        };
        if !observation_ids.insert(observation_id.to_string()) {
            // The Runtime batch guard will reject this later call before execution. Keeping a
            // second authority copy would instead let approval restore bypass that guard.
            queued.file_observation = None;
            continue;
        }
        if pending_file_observation.is_some_and(|pending| pending.observation_id == observation_id)
        {
            queued.file_observation = None;
            continue;
        }
        let checkpoint = registry
            .checkpoint_exact(observation_id, conversation_id, run_id, &canonical_target)
            .map_err(|_| {
                AgentError::new("无法创建运行检查点：queued apply_patch 缺少当前且匹配的文件观察。")
            })?;
        if !source_call_ids.insert(checkpoint.source_tool_call_id.clone()) {
            return Err(AgentError::new(
                "无法创建运行检查点：多个文件观察重复绑定同一来源工具调用。",
            ));
        }
        validate_observation_source(context_items, &checkpoint, run_context, "创建")?;
        queued.file_observation = Some(checkpoint);
    }
    Ok(())
}

fn restore_queued_file_observations(
    queued_calls: &[AgentQueuedToolCallCheckpoint],
    run_id: &str,
    run_context: Option<&AgentRunContext>,
    context_items: &[AgentContextCheckpointItem],
    pending_file_observation: Option<&FileObservationCheckpoint>,
) -> AgentResult<Arc<FileObservationRegistry>> {
    let mut checkpoints = Vec::new();
    let mut observation_ids = pending_file_observation
        .map(|checkpoint| BTreeSet::from([checkpoint.observation_id.clone()]))
        .unwrap_or_default();
    let mut source_call_ids = BTreeSet::new();
    let mut expected_conversation_id = None;
    for queued in queued_calls {
        match (
            queued_apply_patch_observation_request(&queued.call, run_context)?,
            queued.file_observation.as_ref(),
        ) {
            (None, None) => {}
            (None, Some(_)) => {
                return Err(AgentError::new(
                    "无法恢复运行检查点：非 apply_patch 调用包含额外的文件观察。",
                ));
            }
            (Some((observation_id, _, _)), None) if observation_ids.contains(observation_id) => {}
            (Some(_), None) => {
                return Err(AgentError::new(
                    "无法恢复运行检查点：queued apply_patch 缺少文件观察。",
                ));
            }
            (Some((observation_id, canonical_target, conversation_id)), Some(checkpoint)) => {
                if checkpoint.observation_id != observation_id
                    || Path::new(&checkpoint.canonical_target) != canonical_target
                    || checkpoint.conversation_id != conversation_id
                    || checkpoint.run_id != run_id
                    || !observation_ids.insert(checkpoint.observation_id.clone())
                    || !source_call_ids.insert(checkpoint.source_tool_call_id.clone())
                {
                    return Err(AgentError::new(
                        "无法恢复运行检查点：queued apply_patch 的文件观察身份、路径或所有者不匹配。",
                    ));
                }
                match expected_conversation_id {
                    Some(expected) if expected != conversation_id => {
                        return Err(AgentError::new(
                            "无法恢复运行检查点：文件观察属于不同会话。",
                        ));
                    }
                    None => expected_conversation_id = Some(conversation_id),
                    _ => {}
                }
                validate_observation_source(context_items, checkpoint, run_context, "恢复")?;
                checkpoints.push(checkpoint.clone());
            }
        }
    }
    if checkpoints.is_empty() {
        return Ok(Arc::new(FileObservationRegistry::default()));
    }
    let conversation_id = expected_conversation_id.expect("non-empty checkpoints have an owner");
    FileObservationRegistry::from_checkpoints(checkpoints, conversation_id, run_id)
        .map(Arc::new)
        .map_err(|_| AgentError::new("无法恢复运行检查点：文件观察已过期或结构无效。"))
}

fn queued_apply_patch_observation_request<'a>(
    call: &'a AgentContextCheckpointToolCall,
    run_context: Option<&'a AgentRunContext>,
) -> AgentResult<Option<(&'a str, PathBuf, &'a str)>> {
    if call.name != "apply_patch" {
        return Ok(None);
    }
    if !crate::tools::apply_patch_wire_is_valid(&call.args) {
        return Err(AgentError::new(
            "运行检查点中的 queued apply_patch 参数不符合当前严格协议。",
        ));
    }
    let object = crate::tools::apply_patch_request(&call.args)
        .ok_or_else(|| AgentError::new("运行检查点中的 queued apply_patch 参数不是严格对象。"))?;
    let action = object
        .get("action")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AgentError::new("运行检查点中的 queued apply_patch 缺少当前 action。"))?;
    if !matches!(action, "apply" | "begin") {
        return Ok(None);
    }
    let operation = object
        .get("operation")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AgentError::new("运行检查点中的 queued apply_patch 缺少 operation。"))?;
    if operation == "create" {
        return Ok(None);
    }
    let observation_id = object
        .get("observationId")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AgentError::new("运行检查点中的 queued apply_patch 缺少 observationId。"))?;
    let file_path = object
        .get("filePath")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AgentError::new("运行检查点中的 queued apply_patch 缺少 filePath。"))?;
    let run_context = run_context.ok_or_else(|| {
        AgentError::new("运行检查点中的 queued apply_patch 缺少冻结的运行上下文。")
    })?;
    let conversation_id = run_context
        .conversation_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            AgentError::new("运行检查点中的 queued apply_patch 缺少 conversationId。")
        })?;
    let target = FileChangePathPolicy::from_workspace(
        run_context.workspace.as_ref(),
        run_context.permissions.write == AgentWritePermission::All,
    )
    .resolve(file_path)
    .map_err(|_| AgentError::new("运行检查点中的 queued apply_patch 文件路径无法安全解析。"))?;
    Ok(Some((
        observation_id,
        target.absolute_path().to_path_buf(),
        conversation_id,
    )))
}

fn validate_pending_file_observation(
    pending_call: &AgentContextCheckpointToolCall,
    checkpoint: Option<&FileObservationCheckpoint>,
    run_id: &str,
    run_context: Option<&AgentRunContext>,
    context_items: &[AgentContextCheckpointItem],
    operation: &str,
) -> AgentResult<()> {
    let requested = queued_apply_patch_observation_request(pending_call, run_context)?;
    let is_staged_commit = pending_call.name == "apply_patch"
        && crate::tools::apply_patch_request(&pending_call.args)
            .and_then(|request| request.get("action"))
            .and_then(serde_json::Value::as_str)
            == Some("commit");

    match (requested, checkpoint) {
        (None, None) => Ok(()),
        (Some(_), None) => Err(AgentError::new(format!(
            "无法{operation}运行检查点：待审批 FileChange 缺少冻结的文件观察。"
        ))),
        (None, Some(_)) if !is_staged_commit => Err(AgentError::new(format!(
            "无法{operation}运行检查点：当前待审批调用包含不允许的文件观察。"
        ))),
        (requested, Some(checkpoint)) => {
            let run_context = run_context.ok_or_else(|| {
                AgentError::new(format!(
                    "无法{operation}运行检查点：待审批 FileChange 缺少运行上下文。"
                ))
            })?;
            let conversation_id = run_context
                .conversation_id
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    AgentError::new(format!(
                        "无法{operation}运行检查点：待审批 FileChange 缺少会话上下文。"
                    ))
                })?;
            if !matches!(checkpoint.state, FileObservationState::Existing { .. }) {
                return Err(AgentError::new(format!(
                    "无法{operation}运行检查点：待审批 update/delete 的文件观察状态无效。"
                )));
            }
            let target = requested
                .as_ref()
                .map(|(_, target, _)| target.as_path())
                .unwrap_or_else(|| Path::new(&checkpoint.canonical_target));
            checkpoint
                .validate_frozen_binding(conversation_id, run_id, target)
                .map_err(|_| {
                    AgentError::new(format!(
                        "无法{operation}运行检查点：待审批 FileChange 的文件观察无效。"
                    ))
                })?;
            if requested
                .is_some_and(|(observation_id, _, _)| observation_id != checkpoint.observation_id)
            {
                return Err(AgentError::new(format!(
                    "无法{operation}运行检查点：待审批 FileChange 的 observationId 不一致。"
                )));
            }
            validate_observation_source(context_items, checkpoint, Some(run_context), operation)
        }
    }
}

fn validate_observation_source(
    context_items: &[AgentContextCheckpointItem],
    checkpoint: &FileObservationCheckpoint,
    run_context: Option<&AgentRunContext>,
    operation: &str,
) -> AgentResult<()> {
    validate_model_tool_call_id(&checkpoint.source_tool_call_id)?;
    let run_context = run_context.ok_or_else(|| {
        AgentError::new(format!(
            "无法{operation}运行检查点：文件观察缺少冻结的运行上下文。"
        ))
    })?;
    let matching_source_calls = context_items
        .iter()
        .filter(|item| item.role == "assistant")
        .flat_map(|item| item.tool_calls.iter())
        .filter(|call| {
            call.id == checkpoint.source_tool_call_id
                && matches!(call.name.as_str(), "read_file" | "apply_patch")
        })
        .collect::<Vec<_>>();
    let Some(source_call) = matching_source_calls.first().copied() else {
        return Err(AgentError::new(format!(
            "无法{operation}运行检查点：文件观察缺少唯一且已完成的来源工具调用。"
        )));
    };
    let source_call_shape_matches = match source_call.name.as_str() {
        "read_file" => source_call
            .args
            .as_object()
            .and_then(|args| args.get("path"))
            .and_then(serde_json::Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .and_then(|path| resolve_checkpoint_file_target(run_context, path).ok())
            .is_some_and(|target| Path::new(&checkpoint.canonical_target) == target),
        "apply_patch" => crate::tools::apply_patch_wire_is_valid(&source_call.args),
        _ => false,
    };
    let matching_results = context_items
        .iter()
        .filter(|item| {
            item.role == "tool"
                && !item.is_error
                && item.tool_call_id.as_deref() == Some(&checkpoint.source_tool_call_id)
                && serde_json::from_str::<serde_json::Value>(&item.content)
                    .ok()
                    .is_some_and(|value| {
                        observation_result_matches_checkpoint(
                            &value,
                            checkpoint,
                            run_context,
                            source_call,
                            context_items,
                        )
                    })
        })
        .count();
    if matching_source_calls.len() != 1 || !source_call_shape_matches || matching_results != 1 {
        return Err(AgentError::new(format!(
            "无法{operation}运行检查点：文件观察没有绑定唯一、同路径且已完成的 read_file 或 apply_patch 调用。"
        )));
    }
    Ok(())
}

fn observation_result_matches_checkpoint(
    value: &serde_json::Value,
    checkpoint: &FileObservationCheckpoint,
    run_context: &AgentRunContext,
    source_call: &AgentContextCheckpointToolCall,
    context_items: &[AgentContextCheckpointItem],
) -> bool {
    if source_call.name == "apply_patch" {
        return apply_patch_observation_result_matches_checkpoint(
            value,
            checkpoint,
            run_context,
            source_call,
            context_items,
        );
    }
    if source_call.name != "read_file" {
        return false;
    }
    let Some(result) = value.as_object() else {
        return false;
    };
    if result
        .get("observationId")
        .and_then(serde_json::Value::as_str)
        != Some(&checkpoint.observation_id)
    {
        return false;
    }
    let Some(result_target) = result
        .get("path")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .and_then(|path| resolve_checkpoint_file_target(run_context, path).ok())
    else {
        return false;
    };
    if result_target != Path::new(&checkpoint.canonical_target) {
        return false;
    }
    match &checkpoint.state {
        FileObservationState::Missing => {
            result.get("exists").and_then(serde_json::Value::as_bool) == Some(false)
                && !result.contains_key("revision")
        }
        FileObservationState::Existing { revision, .. } => {
            result.get("exists").and_then(serde_json::Value::as_bool) == Some(true)
                && result.get("revision").and_then(serde_json::Value::as_str) == Some(revision)
        }
    }
}

fn apply_patch_observation_result_matches_checkpoint(
    value: &serde_json::Value,
    checkpoint: &FileObservationCheckpoint,
    run_context: &AgentRunContext,
    source_call: &AgentContextCheckpointToolCall,
    context_items: &[AgentContextCheckpointItem],
) -> bool {
    let Some(result) = value.as_object() else {
        return false;
    };
    if result
        .get("observationId")
        .and_then(serde_json::Value::as_str)
        != Some(&checkpoint.observation_id)
    {
        return false;
    }
    let Some(target) = result
        .get("fileChangeTarget")
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    let target_keys = target.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if target_keys != BTreeSet::from(["filePath", "observationId", "state"])
        || target
            .get("observationId")
            .and_then(serde_json::Value::as_str)
            != Some(&checkpoint.observation_id)
    {
        return false;
    }
    let Some(file_path) = target
        .get("filePath")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.trim().is_empty())
    else {
        return false;
    };
    let Some(canonical_target) = resolve_checkpoint_file_target(run_context, file_path).ok() else {
        return false;
    };
    if canonical_target != Path::new(&checkpoint.canonical_target) {
        return false;
    }

    let mut terminal = value.clone();
    let Some(terminal_object) = terminal.as_object_mut() else {
        return false;
    };
    terminal_object.remove("observationId");
    terminal_object.remove("fileChangeTarget");
    let Ok(terminal) = serde_json::from_value::<crate::protocol::AgentFileChangeResult>(terminal)
    else {
        return false;
    };
    if !matches!(
        terminal.status,
        crate::protocol::AgentFileChangeResultStatus::Applied
            | crate::protocol::AgentFileChangeResultStatus::AlreadyApplied
    ) || terminal.file_path != file_path
    {
        return false;
    }
    let Some(request) = crate::tools::apply_patch_request(&source_call.args) else {
        return false;
    };
    let terminal_operation = match terminal.operation {
        crate::protocol::AgentFileChangeOperation::Create => "create",
        crate::protocol::AgentFileChangeOperation::Update => "update",
        crate::protocol::AgentFileChangeOperation::Delete => "delete",
    };
    let call_matches = match request.get("action").and_then(serde_json::Value::as_str) {
        Some("apply") => {
            request.get("filePath").and_then(serde_json::Value::as_str) == Some(file_path)
                && request.get("operation").and_then(serde_json::Value::as_str)
                    == Some(terminal_operation)
        }
        Some("commit") => {
            request
                .get("transactionId")
                .and_then(serde_json::Value::as_str)
                == Some(terminal.transaction_id.as_str())
                && staged_begin_source_matches(
                    context_items,
                    run_context,
                    &terminal,
                    &source_call.id,
                )
        }
        _ => false,
    };
    if !call_matches {
        return false;
    }
    match (&checkpoint.state, terminal.operation) {
        (FileObservationState::Missing, crate::protocol::AgentFileChangeOperation::Delete) => {
            target.get("state").and_then(serde_json::Value::as_str) == Some("missing")
                && terminal.revision.is_none()
        }
        (
            FileObservationState::Existing { revision, .. },
            crate::protocol::AgentFileChangeOperation::Create
            | crate::protocol::AgentFileChangeOperation::Update,
        ) => {
            target.get("state").and_then(serde_json::Value::as_str) == Some("existing")
                && terminal.revision.as_deref() == Some(revision)
        }
        _ => false,
    }
}

fn staged_begin_source_matches(
    context_items: &[AgentContextCheckpointItem],
    run_context: &AgentRunContext,
    terminal: &crate::protocol::AgentFileChangeResult,
    commit_call_id: &str,
) -> bool {
    let commit_item_indices = context_items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.role == "assistant"
                && item
                    .tool_calls
                    .iter()
                    .any(|call| call.id == commit_call_id && call.name == "apply_patch")
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let [commit_item_index] = commit_item_indices.as_slice() else {
        return false;
    };
    let terminal_target = resolve_checkpoint_file_target(run_context, &terminal.file_path).ok();
    let matches = context_items
        .iter()
        .enumerate()
        .filter(|(index, _)| index < commit_item_index)
        .filter(|(_, item)| item.role == "assistant")
        .flat_map(|(index, item)| item.tool_calls.iter().map(move |call| (index, call)))
        .filter(|(_, call)| call.name == "apply_patch")
        .filter(|(begin_item_index, call)| {
            let Some(request) = crate::tools::apply_patch_request(&call.args) else {
                return false;
            };
            if request.get("action").and_then(serde_json::Value::as_str) != Some("begin")
                || !crate::tools::apply_patch_wire_is_valid(&call.args)
            {
                return false;
            }
            let call_target = request
                .get("filePath")
                .and_then(serde_json::Value::as_str)
                .and_then(|path| resolve_checkpoint_file_target(run_context, path).ok());
            let operation = request.get("operation").and_then(serde_json::Value::as_str);
            let strategy = request.get("strategy").and_then(serde_json::Value::as_str);
            let terminal_strategy = match terminal.update_strategy {
                Some(crate::protocol::AgentFileChangeUpdateStrategy::Modify) => Some("modify"),
                Some(crate::protocol::AgentFileChangeUpdateStrategy::Rewrite) => Some("rewrite"),
                None => None,
            };
            let operation_matches = match terminal.operation {
                crate::protocol::AgentFileChangeOperation::Create => {
                    operation == Some("create")
                        && strategy.is_none()
                        && terminal.update_strategy.is_none()
                }
                crate::protocol::AgentFileChangeOperation::Update => {
                    operation == Some("update") && strategy == terminal_strategy
                }
                crate::protocol::AgentFileChangeOperation::Delete => false,
            };
            if call_target != terminal_target || !operation_matches {
                return false;
            }
            context_items
                .iter()
                .enumerate()
                .filter(|(index, _)| index > begin_item_index && index < commit_item_index)
                .filter(|(_, item)| {
                    item.role == "tool"
                        && !item.is_error
                        && item.tool_call_id.as_deref() == Some(call.id.as_str())
                })
                .filter_map(|(_, item)| {
                    serde_json::from_str::<serde_json::Value>(&item.content).ok()
                })
                .filter(|result| {
                    staged_begin_result_matches_current_shape(result, terminal, operation, strategy)
                })
                .count()
                == 1
        })
        .count();
    matches == 1
}

fn staged_begin_result_matches_current_shape(
    result: &serde_json::Value,
    terminal: &crate::protocol::AgentFileChangeResult,
    operation: Option<&str>,
    strategy: Option<&str>,
) -> bool {
    let Some(result) = result.as_object() else {
        return false;
    };
    let keys = result.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if keys
        != BTreeSet::from([
            "additions",
            "allowedNextActions",
            "byteCount",
            "deletions",
            "draftRevision",
            "filePath",
            "lineCount",
            "mutationCount",
            "nextIndex",
            "operation",
            "requiresCommitBeforeResponse",
            "status",
            "strategy",
            "tail",
            "tailStart",
            "tailTruncated",
            "totalChars",
            "transactionId",
        ])
        || result
            .get("transactionId")
            .and_then(serde_json::Value::as_str)
            != Some(terminal.transaction_id.as_str())
        || result.get("filePath").and_then(serde_json::Value::as_str)
            != Some(terminal.file_path.as_str())
        || result.get("operation").and_then(serde_json::Value::as_str) != operation
        || result.get("strategy").and_then(serde_json::Value::as_str) != strategy
        || result.get("status").and_then(serde_json::Value::as_str) != Some("drafting")
        || result
            .get("draftRevision")
            .and_then(serde_json::Value::as_u64)
            != Some(0)
        || result.get("nextIndex").and_then(serde_json::Value::as_u64) != Some(0)
        || result
            .get("mutationCount")
            .and_then(serde_json::Value::as_u64)
            != Some(0)
        || result
            .get("requiresCommitBeforeResponse")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        || !matches!(result.get("tail"), Some(serde_json::Value::String(_)))
        || ![
            "byteCount",
            "lineCount",
            "additions",
            "deletions",
            "totalChars",
            "tailStart",
        ]
        .iter()
        .all(|key| {
            result
                .get(*key)
                .and_then(serde_json::Value::as_u64)
                .is_some()
        })
        || !matches!(
            result.get("allowedNextActions"),
            Some(serde_json::Value::Array(actions))
                if actions == &[
                    serde_json::Value::String("append".to_string()),
                    serde_json::Value::String("edit".to_string()),
                    serde_json::Value::String("commit".to_string()),
                    serde_json::Value::String("status".to_string()),
                    serde_json::Value::String("abort".to_string()),
                ]
        )
    {
        return false;
    }
    result
        .get("tailStart")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|tail_start| {
            result
                .get("tailTruncated")
                .and_then(serde_json::Value::as_bool)
                == Some(tail_start > 0)
        })
}

fn resolve_checkpoint_file_target(
    run_context: &AgentRunContext,
    file_path: &str,
) -> Result<PathBuf, crate::file_change::FileChangeError> {
    FileChangePathPolicy::from_workspace(
        run_context.workspace.as_ref(),
        run_context.permissions.write == AgentWritePermission::All,
    )
    .resolve(file_path)
    .map(|target| target.absolute_path().to_path_buf())
}

#[cfg(test)]
mod multi_workspace_observation_tests {
    use super::*;
    use crate::file_change::FileChangeDirectoryIdentity;
    use crate::storage::models::ProjectFolderRole;
    use crate::workspace::WorkspaceFolder;
    use crate::{AgentPermissions, AgentWorkspaceContext};

    #[test]
    fn multi_workspace_queued_observation_restore_uses_frozen_roots_and_rejects_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("main");
        let docs = temp.path().join("docs");
        std::fs::create_dir(&main).unwrap();
        std::fs::create_dir(&docs).unwrap();
        let main = main.canonicalize().unwrap();
        let docs = docs.canonicalize().unwrap();
        let context = AgentRunContext {
            conversation_id: Some("conversation-multi".into()),
            project_id: Some("project-multi".into()),
            workspace: Some(AgentWorkspaceContext {
                project_id: Some("project-multi".into()),
                display_name: None,
                root_path: Some(main.to_string_lossy().into_owned()),
                folders: [
                    (&main, "main", ProjectFolderRole::Primary),
                    (&docs, "docs", ProjectFolderRole::Auxiliary),
                ]
                .into_iter()
                .map(|(path, alias, role)| WorkspaceFolder {
                    id: format!("folder-{alias}"),
                    alias: alias.into(),
                    role,
                    path: path.to_string_lossy().into_owned(),
                    canonical_path: Some(path.to_string_lossy().into_owned()),
                    directory_identity: Some(FileChangeDirectoryIdentity::read(path).unwrap()),
                })
                .collect(),
            }),
            permissions: AgentPermissions::default(),
            attachment_library: None,
            collaboration_identity: None,
        };
        let frozen: AgentRunContext =
            serde_json::from_str(&serde_json::to_string(&context).unwrap()).unwrap();
        let call = AgentContextCheckpointToolCall {
            id: "update-aux".into(),
            name: "apply_patch".into(),
            args: serde_json::json!({"request": {"action":"apply", "operation":"update", "filePath":"@workspace/docs/README.md", "observationId":"fobs_checkpoint", "content":"new\n"}}),
            provider_identity: AgentProviderToolCallIdentity {
                provider_tool_index: 0,
                provider_call_id: "update-aux".into(),
                runtime_call_id: "update-aux".into(),
            },
        };
        let (observation, target, conversation) =
            queued_apply_patch_observation_request(&call, Some(&frozen))
                .unwrap()
                .unwrap();
        assert_eq!(observation, "fobs_checkpoint");
        assert_eq!(target, docs.join("README.md"));
        assert_eq!(conversation, "conversation-multi");
        assert_eq!(
            resolve_checkpoint_file_target(&frozen, "README.md").unwrap(),
            main.join("README.md")
        );
        std::fs::rename(&docs, temp.path().join("old-docs")).unwrap();
        std::fs::create_dir(&docs).unwrap();
        assert!(queued_apply_patch_observation_request(&call, Some(&frozen)).is_err());
        assert!(resolve_checkpoint_file_target(&frozen, "README.md").is_ok());
    }
}
