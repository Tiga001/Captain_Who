use super::*;

pub(super) fn mutation_from_args(
    operation: FileChangeOperation,
    content: Option<String>,
    edits: Option<Vec<StructuredTextEdit>>,
) -> Result<FileChangeMutation, FileChangeError> {
    match operation {
        FileChangeOperation::Create | FileChangeOperation::Update if content.is_some() => {
            let content = content.expect("checked content presence");
            if content.len() > MAX_INLINE_CONTENT_BYTES {
                return Err(FileChangeError::new(FileChangeErrorCode::ContentTooLarge));
            }
            Ok(FileChangeMutation::Complete(content))
        }
        FileChangeOperation::Update => Ok(FileChangeMutation::Edits(
            edits
                .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::InvalidArguments))?
                .into_iter()
                .map(domain_edit)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        FileChangeOperation::Delete => Ok(FileChangeMutation::Delete),
        FileChangeOperation::Create => Err(FileChangeError::new(
            FileChangeErrorCode::IllegalFieldCombination,
        )),
    }
}

pub(super) fn domain_edit(edit: StructuredTextEdit) -> Result<FileChangeEdit, FileChangeError> {
    let invalid = || FileChangeError::new(FileChangeErrorCode::IllegalFieldCombination);
    match edit.kind {
        TextEditKind::Replace => {
            if edit.anchor.is_some() || edit.text.is_some() {
                return Err(invalid());
            }
            Ok(FileChangeEdit::Replace {
                old_text: edit.old_text.ok_or_else(invalid)?,
                new_text: edit.new_text.ok_or_else(invalid)?,
                replace_all: edit.replace_all.unwrap_or(false),
            })
        }
        TextEditKind::InsertBefore | TextEditKind::InsertAfter => {
            if edit.old_text.is_some() || edit.new_text.is_some() || edit.replace_all.is_some() {
                return Err(invalid());
            }
            let anchor = edit.anchor.ok_or_else(invalid)?;
            let text = edit.text.ok_or_else(invalid)?;
            if matches!(edit.kind, TextEditKind::InsertBefore) {
                Ok(FileChangeEdit::InsertBefore { anchor, text })
            } else {
                Ok(FileChangeEdit::InsertAfter { anchor, text })
            }
        }
        TextEditKind::Append | TextEditKind::Prepend => {
            if edit.old_text.is_some()
                || edit.new_text.is_some()
                || edit.anchor.is_some()
                || edit.replace_all.is_some()
            {
                return Err(invalid());
            }
            let text = edit.text.ok_or_else(invalid)?;
            if matches!(edit.kind, TextEditKind::Append) {
                Ok(FileChangeEdit::Append { text })
            } else {
                Ok(FileChangeEdit::Prepend { text })
            }
        }
    }
}

pub(in crate::tools) fn validate_observation_operation(
    operation: FileChangeOperation,
    state: &FileObservationState,
) -> Result<(), FileChangeError> {
    match (operation, state) {
        (FileChangeOperation::Create, FileObservationState::Missing)
        | (
            FileChangeOperation::Update | FileChangeOperation::Delete,
            FileObservationState::Existing { .. },
        ) => Ok(()),
        (FileChangeOperation::Create, FileObservationState::Existing { .. }) => {
            Err(FileChangeError::new(FileChangeErrorCode::FileExists))
        }
        (
            FileChangeOperation::Update | FileChangeOperation::Delete,
            FileObservationState::Missing,
        ) => Err(FileChangeError::new(FileChangeErrorCode::FileMissing)),
    }
}

pub(in crate::tools) fn capture_missing_base_observation(
    context: &ToolExecutionContext,
    target: &crate::file_change::ResolvedFileChangeTarget,
) -> Result<crate::file_change::FileObservation, FileChangeError> {
    context.check_cancelled().map_err(|error| {
        FileChangeError::with_diagnostic(FileChangeErrorCode::Failed, error.to_string())
    })?;
    let parent = BoundParent::open(target)?;
    if parent.read_optional(target.absolute_path())?.is_some() {
        return Err(FileChangeError::new(FileChangeErrorCode::FileExists));
    }
    parent.revalidate()?;
    let parent_metadata = parent.parent_metadata()?;
    context.file_observations().capture_missing(
        context.conversation_id().map_err(|error| {
            FileChangeError::with_diagnostic(FileChangeErrorCode::Failed, error.to_string())
        })?,
        context.run_id().map_err(|error| {
            FileChangeError::with_diagnostic(FileChangeErrorCode::Failed, error.to_string())
        })?,
        target.absolute_path(),
        &parent_metadata,
    )
}

#[derive(Debug)]
pub(in crate::tools) struct FrozenBase {
    pub(in crate::tools) content: Option<String>,
    pub(in crate::tools) revision: Option<String>,
}

impl FrozenBase {
    pub(super) fn as_plan_base(&self) -> FileChangeBase<'_> {
        match (&self.content, &self.revision) {
            (None, None) => FileChangeBase::Missing,
            (Some(content), Some(revision)) => FileChangeBase::Existing { content, revision },
            _ => unreachable!("frozen base content and revision are total"),
        }
    }
}

pub(super) fn freeze_observed_base(
    context: &ToolExecutionContext,
    target: &crate::file_change::ResolvedFileChangeTarget,
    observation: &crate::file_change::FileObservation,
) -> Result<FrozenBase, FileChangeError> {
    freeze_observed_base_with_limit_and_hook(
        context,
        target,
        observation,
        MAX_EDIT_CONTENT_BYTES,
        || {},
    )
}

pub(in crate::tools) fn freeze_staged_observed_base(
    context: &ToolExecutionContext,
    target: &crate::file_change::ResolvedFileChangeTarget,
    observation: &crate::file_change::FileObservation,
    max_bytes: usize,
) -> Result<FrozenBase, FileChangeError> {
    freeze_observed_base_with_limit_and_hook(context, target, observation, max_bytes, || {})
}

#[cfg(test)]
pub(super) fn freeze_observed_base_with_hook(
    context: &ToolExecutionContext,
    target: &crate::file_change::ResolvedFileChangeTarget,
    observation: &crate::file_change::FileObservation,
    after_parent_bind: impl FnOnce(),
) -> Result<FrozenBase, FileChangeError> {
    freeze_observed_base_with_limit_and_hook(
        context,
        target,
        observation,
        MAX_EDIT_CONTENT_BYTES,
        after_parent_bind,
    )
}

fn freeze_observed_base_with_limit_and_hook(
    context: &ToolExecutionContext,
    target: &crate::file_change::ResolvedFileChangeTarget,
    observation: &crate::file_change::FileObservation,
    max_bytes: usize,
    after_parent_bind: impl FnOnce(),
) -> Result<FrozenBase, FileChangeError> {
    context.check_cancelled().map_err(|error| {
        FileChangeError::with_diagnostic(FileChangeErrorCode::Failed, error.to_string())
    })?;
    let parent = BoundParent::open(target)?;
    after_parent_bind();
    let current = parent.read_optional(target.absolute_path())?;
    // The held descriptor prevents redirection; this pathname revalidation only proves that the
    // authorized target still names that descriptor before a proposal can expose its Diff.
    parent.revalidate()?;
    let parent_metadata = parent.parent_metadata()?;
    if !observation.parent_identity_matches(target.absolute_path(), &parent_metadata)? {
        return Err(FileChangeError::new(FileChangeErrorCode::ObservationStale));
    }
    match (observation.state(), current) {
        (FileObservationState::Missing, None) => Ok(FrozenBase {
            content: None,
            revision: None,
        }),
        (FileObservationState::Missing, Some(_)) => {
            Err(FileChangeError::new(FileChangeErrorCode::FileExists))
        }
        (FileObservationState::Existing { .. }, None) => {
            Err(FileChangeError::new(FileChangeErrorCode::FileMissing))
        }
        (FileObservationState::Existing { revision, identity }, Some(current)) => {
            if FileObservationIdentity::from_metadata(&current.metadata) != *identity {
                return Err(FileChangeError::new(FileChangeErrorCode::ObservationStale));
            }
            if current.bytes.len() > max_bytes {
                return Err(FileChangeError::new(FileChangeErrorCode::ContentTooLarge));
            }
            let content = String::from_utf8(current.bytes).map_err(|error| {
                FileChangeError::with_diagnostic(
                    FileChangeErrorCode::UnsupportedFileType,
                    error.to_string(),
                )
            })?;
            let actual_revision = content_revision(content.as_bytes());
            if actual_revision != *revision {
                return Err(FileChangeError::new(FileChangeErrorCode::ObservationStale));
            }
            Ok(FrozenBase {
                content: Some(content),
                revision: Some(actual_revision),
            })
        }
    }
}

pub(in crate::tools) fn patch_operation(
    operation: FileChangeOperation,
) -> AgentFileChangeOperation {
    match operation {
        FileChangeOperation::Create => AgentFileChangeOperation::Create,
        FileChangeOperation::Update => AgentFileChangeOperation::Update,
        FileChangeOperation::Delete => AgentFileChangeOperation::Delete,
    }
}

pub(in crate::tools) fn state_revision(state: &FileChangeContentState) -> Option<&str> {
    match state {
        FileChangeContentState::Missing => None,
        FileChangeContentState::Present { revision, .. } => Some(revision),
    }
}

pub(in crate::tools) fn sanitize_summary(summary: Option<String>) -> Option<String> {
    summary
        .map(|summary| summary.trim().chars().take(MAX_SUMMARY_CHARS).collect())
        .filter(|summary: &String| !summary.is_empty())
}

pub(in crate::tools) fn file_change_agent_error(error: FileChangeError) -> AgentError {
    file_change_agent_error_with_continuation(error, None, None)
}

#[derive(Debug, Clone, Copy)]
pub(super) enum DirectStagedRecovery {
    Create,
    Modify,
    Rewrite,
}

pub(super) fn file_change_wire_error(error: FileChangeError, value: &Value) -> AgentError {
    let continuation = wire_safe_continuation(error.code(), value);
    file_change_agent_error_with_continuation(error, continuation, Some(wire_expected_shape(value)))
}

fn wire_safe_continuation(code: FileChangeErrorCode, value: &Value) -> Option<Value> {
    let request = apply_patch_request(value)?;
    let action = request.get("action").and_then(Value::as_str)?;
    if matches!(action, "apply" | "begin") {
        let file_path = bounded_nonempty_string(request, "filePath", 4_096)?;
        let operation = request.get("operation").and_then(Value::as_str);
        let observation_is_invalid = request
            .get("observationId")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty);
        if observation_is_invalid && operation != Some("create") {
            return Some(read_file_continuation(file_path));
        }
        if code == FileChangeErrorCode::ContentTooLarge && action == "apply" {
            let observation_id = request
                .get("observationId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty() && value.chars().count() <= 256);
            let staged_recovery = match operation {
                Some("create") if request.contains_key("content") => {
                    Some(DirectStagedRecovery::Create)
                }
                Some("update") if request.contains_key("content") => {
                    Some(DirectStagedRecovery::Rewrite)
                }
                Some("update") if request.contains_key("edits") => {
                    Some(DirectStagedRecovery::Modify)
                }
                _ => None,
            };
            return staged_recovery
                .map(|recovery| direct_staged_continuation(file_path, observation_id, recovery));
        }
        return None;
    }
    if matches!(action, "append" | "edit" | "commit") {
        let transaction_id = bounded_nonempty_string(request, "transactionId", 256)?;
        if request_has_invalid_cursor(action, request) {
            return Some(staged_status_continuation(transaction_id));
        }
    }
    None
}

fn bounded_nonempty_string<'a>(
    request: &'a Map<String, Value>,
    key: &str,
    max_chars: usize,
) -> Option<&'a str> {
    request
        .get(key)?
        .as_str()
        .filter(|value| !value.is_empty() && value.chars().count() <= max_chars)
}

fn request_has_invalid_cursor(action: &str, request: &Map<String, Value>) -> bool {
    let expected_revision_invalid = request
        .get("expectedDraftRevision")
        .is_none_or(|value| value.as_u64().is_none());
    let index_invalid = matches!(action, "append" | "edit")
        && request
            .get("index")
            .is_none_or(|value| value.as_u64().is_none());
    let forbidden_cursor_field = ["nextIndex", "draftRevision", "expected_draft_revision"]
        .iter()
        .any(|key| request.contains_key(*key))
        || (action == "commit" && request.contains_key("index"));
    expected_revision_invalid || index_invalid || forbidden_cursor_field
}

pub(super) fn file_change_agent_error_for_direct(
    error: FileChangeError,
    file_path: &str,
    observation_id: Option<&str>,
    staged_recovery: Option<DirectStagedRecovery>,
) -> AgentError {
    let continuation = if error.code() == FileChangeErrorCode::ContentTooLarge {
        staged_recovery
            .map(|recovery| direct_staged_continuation(file_path, observation_id, recovery))
    } else if error_requires_reread(error.code()) {
        Some(read_file_continuation(file_path))
    } else {
        None
    };
    file_change_agent_error_with_continuation(error, continuation, None)
}

pub(in crate::tools) fn file_change_agent_error_for_path(
    error: FileChangeError,
    file_path: &str,
) -> AgentError {
    let continuation =
        error_requires_reread(error.code()).then(|| read_file_continuation(file_path));
    file_change_agent_error_with_continuation(error, continuation, None)
}

pub(in crate::tools) fn file_change_agent_error_for_transaction(
    error: FileChangeError,
    transaction_id: &str,
) -> AgentError {
    let continuation = matches!(
        error.code(),
        FileChangeErrorCode::DraftRevisionConflict | FileChangeErrorCode::MutationOutOfOrder
    )
    .then(|| staged_status_continuation(transaction_id));
    file_change_agent_error_with_continuation(error, continuation, None)
}

fn error_requires_reread(code: FileChangeErrorCode) -> bool {
    matches!(
        code,
        FileChangeErrorCode::FileExists
            | FileChangeErrorCode::FileMissing
            | FileChangeErrorCode::RevisionConflict
            | FileChangeErrorCode::MatchNotFound
            | FileChangeErrorCode::AmbiguousMatch
            | FileChangeErrorCode::ObservationRequired
            | FileChangeErrorCode::ObservationExpired
            | FileChangeErrorCode::ObservationOwnerMismatch
            | FileChangeErrorCode::ObservationPathMismatch
            | FileChangeErrorCode::ObservationStale
    )
}

pub(super) fn read_file_continuation(file_path: &str) -> Value {
    json!({
        "tool": "read_file",
        "args": { "path": file_path }
    })
}

fn direct_staged_continuation(
    file_path: &str,
    observation_id: Option<&str>,
    recovery: DirectStagedRecovery,
) -> Value {
    let mut request = Map::from_iter([
        ("action".to_string(), json!("begin")),
        (
            "operation".to_string(),
            json!(match recovery {
                DirectStagedRecovery::Create => "create",
                DirectStagedRecovery::Modify | DirectStagedRecovery::Rewrite => "update",
            }),
        ),
        ("filePath".to_string(), json!(file_path)),
    ]);
    match recovery {
        DirectStagedRecovery::Create => {}
        DirectStagedRecovery::Modify => {
            let observation_id =
                observation_id.expect("update staged recovery always has a validated observation");
            request.insert("observationId".to_string(), json!(observation_id));
            request.insert("strategy".to_string(), json!("modify"));
        }
        DirectStagedRecovery::Rewrite => {
            let observation_id =
                observation_id.expect("update staged recovery always has a validated observation");
            request.insert("observationId".to_string(), json!(observation_id));
            request.insert("strategy".to_string(), json!("rewrite"));
        }
    }
    json!({
        "tool": "apply_patch",
        "args": { "request": request }
    })
}

fn staged_status_continuation(transaction_id: &str) -> Value {
    json!({
        "tool": "apply_patch",
        "args": {
            "request": {
                "action": "status",
                "transactionId": transaction_id
            }
        }
    })
}

pub(super) fn wire_expected_shape(value: &Value) -> Value {
    let root = json!({
        "requiredFields": ["request"],
        "allowedFields": ["request"]
    });
    let Some(request) = apply_patch_request(value) else {
        return json!({
            "root": root,
            "request": {
                "allowedActions": ["apply", "begin", "append", "edit", "commit", "status", "abort"]
            }
        });
    };
    // A malformed call may omit the discriminator while still making exactly one branch clear.
    // Infer only those unambiguous literals so the correction identifies the concrete current
    // shape without echoing private content/edits back through the error channel.
    let action = request
        .get("action")
        .and_then(Value::as_str)
        .or_else(|| inferred_action_for_expected_shape(request));
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .or_else(|| inferred_operation_for_expected_shape(action, request));
    let (required, optional, constraints): (Vec<&str>, Vec<&str>, Vec<&str>) = match (
        action, operation,
    ) {
        (Some("apply"), Some("create")) => (
            vec!["action", "operation", "filePath", "content"],
            vec!["summary"],
            vec![
                "content is complete UTF-8 text and at most 32 KiB",
                "do not include observationId; create is atomic no-clobber",
            ],
        ),
        (Some("apply"), Some("update")) if request.contains_key("edits") => (
            vec!["action", "operation", "filePath", "observationId", "edits"],
            vec!["summary"],
            vec!["do not include content; use exactly one edit shape"],
        ),
        (Some("apply"), Some("update")) => (
            vec![
                "action",
                "operation",
                "filePath",
                "observationId",
                "content",
            ],
            vec!["summary"],
            vec!["choose exactly one of content or edits; content is at most 32 KiB"],
        ),
        (Some("apply"), Some("delete")) => (
            vec!["action", "operation", "filePath", "observationId"],
            vec!["summary"],
            vec!["do not include content or edits"],
        ),
        (Some("begin"), Some("create")) => (
            vec!["action", "operation", "filePath"],
            vec![],
            vec!["do not include observationId, strategy, content, edits, or summary"],
        ),
        (Some("begin"), Some("update")) => (
            vec![
                "action",
                "operation",
                "filePath",
                "observationId",
                "strategy",
            ],
            vec![],
            vec!["strategy is modify or rewrite; do not include content or edits"],
        ),
        (Some("append"), _) => (
            vec![
                "action",
                "transactionId",
                "index",
                "expectedDraftRevision",
                "content",
            ],
            vec![],
            vec!["content is non-empty and at most 1 MiB; transaction total is at most 4 MiB"],
        ),
        (Some("edit"), _) => (
            vec![
                "action",
                "transactionId",
                "index",
                "expectedDraftRevision",
                "edits",
            ],
            vec![],
            vec!["do not include content; every edit uses exactly one listed edit shape"],
        ),
        (Some("commit"), _) => (
            vec!["action", "transactionId", "expectedDraftRevision"],
            vec!["summary"],
            vec![],
        ),
        (Some("status" | "abort"), _) => (vec!["action", "transactionId"], vec![], vec![]),
        _ => {
            return json!({
                "root": root,
                "request": {
                    "allowedActions": ["apply", "begin", "append", "edit", "commit", "status", "abort"],
                    "applyOperations": ["create", "update", "delete"],
                    "beginOperations": ["create", "update"]
                }
            });
        }
    };
    let mut allowed = required.clone();
    allowed.extend(optional.iter().copied());
    json!({
        "root": root,
        "request": {
            "action": action,
            "operation": operation,
            "requiredFields": required,
            "optionalFields": optional,
            "allowedFields": allowed,
            "constraints": constraints,
            "editShapes": {
                "replace": { "requiredFields": ["kind", "oldText", "newText"], "optionalFields": ["replaceAll"] },
                "insert_before": { "requiredFields": ["kind", "anchor", "text"] },
                "insert_after": { "requiredFields": ["kind", "anchor", "text"] },
                "append": { "requiredFields": ["kind", "text"] },
                "prepend": { "requiredFields": ["kind", "text"] }
            }
        }
    })
}

fn inferred_action_for_expected_shape(request: &Map<String, Value>) -> Option<&'static str> {
    match request.get("operation").and_then(Value::as_str) {
        Some("delete") => Some("apply"),
        Some("create" | "update")
            if request.contains_key("content")
                || request.contains_key("edits")
                || request.contains_key("summary") =>
        {
            Some("apply")
        }
        Some("create" | "update") if request.contains_key("strategy") => Some("begin"),
        Some("create")
            if request.contains_key("filePath") && request.contains_key("observationId") =>
        {
            Some("begin")
        }
        _ => None,
    }
}

fn inferred_operation_for_expected_shape(
    action: Option<&str>,
    request: &Map<String, Value>,
) -> Option<&'static str> {
    match action {
        Some("apply") if request.contains_key("edits") => Some("update"),
        Some("apply")
            if !request.contains_key("content")
                && !request.contains_key("edits")
                && request.contains_key("filePath")
                && request.contains_key("observationId") =>
        {
            Some("delete")
        }
        Some("begin") if request.contains_key("strategy") => Some("update"),
        Some("begin")
            if request.contains_key("filePath") && request.contains_key("observationId") =>
        {
            Some("create")
        }
        _ => None,
    }
}

pub(super) fn file_change_agent_error_with_continuation(
    error: FileChangeError,
    continuation: Option<Value>,
    expected_shape: Option<Value>,
) -> AgentError {
    let code = serde_json::to_value(error.code())
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "failed".to_string());
    AgentError::structured(
        format!("agent.apply_patch.{code}"),
        error.to_string(),
        json!({
            "type": "file_change",
            "code": error.failure().code,
            "category": error.failure().category,
            "message": error.failure().message,
            "recovery": error.failure().recovery,
            "continueWith": continuation.unwrap_or(Value::Null),
            "expectedShape": expected_shape.unwrap_or(Value::Null)
        }),
    )
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
