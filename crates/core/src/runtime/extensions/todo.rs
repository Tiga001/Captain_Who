use super::{
    ExtensionDescriptor, ModelRequestContext, ModelRequestPurpose, RuntimeEffect, RuntimeExtension,
    RuntimeExtensionEvent,
};
use crate::context::{ContextItem, ContextRetention, ContextScope, ContextSource};
use crate::llm::LlmMessageRole;
use crate::protocol::{
    AgentError, AgentEvent, AgentResult, AgentTodoItem, AgentTodoState, AgentTodoStatus,
    AgentToolApprovalMode, AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use crate::tools::{AgentTool, ToolExecutionContext};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use unicode_segmentation::UnicodeSegmentation;

const TODO_EXTENSION_ID: &str = "todo";
const TODO_EXTENSION_VERSION: u32 = 1;
const TODO_TOOL_NAME: &str = "todo_update";
const MAX_TODO_ITEMS: usize = 12;
const MAX_TODO_ID_CHARS: usize = 64;
const MAX_TODO_TITLE_CHARS: usize = 120;
const MAX_TODO_NOTE_CHARS: usize = 240;
const MAX_TODO_EXPLANATION_CHARS: usize = 400;
// Bounds the model reminder, not the authoritative plan or the ability to finish a task.
const TODO_CONTEXT_MAX_TOKENS: u64 = 500;

#[cfg(test)]
#[path = "todo_budget_tests.rs"]
mod budget_tests;

pub(super) struct TodoExtension {
    run_id: String,
    state: TodoStateHandle,
}

#[derive(Clone)]
pub(super) struct TodoStateHandle {
    inner: Arc<Mutex<TodoStateStore>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TodoStateStore {
    state: AgentTodoState,
    next_item_id: u64,
}

impl TodoExtension {
    pub(super) fn new(run_id: String) -> (Self, TodoStateHandle) {
        let state = TodoStateHandle {
            inner: Arc::new(Mutex::new(TodoStateStore::new())),
        };
        (
            Self {
                run_id,
                state: state.clone(),
            },
            state,
        )
    }

    fn todo_snapshot_item(&self) -> Option<ContextItem> {
        let state = self.state.state();
        if state.items.is_empty() {
            return None;
        }

        Some(ContextItem::text(
            LlmMessageRole::User,
            render_todo_state(&state),
            ContextSource::RuntimeTodo,
            ContextScope::Run,
            ContextRetention::RequestOnly,
        ))
    }
}

impl RuntimeExtension for TodoExtension {
    fn descriptor(&self) -> ExtensionDescriptor {
        ExtensionDescriptor {
            id: TODO_EXTENSION_ID,
            version: TODO_EXTENSION_VERSION,
            order: 100,
        }
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![Box::new(TodoTool {
            state: self.state.clone(),
        })]
    }

    fn request_context(&self, request: &ModelRequestContext) -> AgentResult<Vec<ContextItem>> {
        if request.purpose == ModelRequestPurpose::ContextCompaction {
            return Ok(Vec::new());
        }
        Ok(self.todo_snapshot_item().into_iter().collect())
    }

    fn on_event(&mut self, event: &RuntimeExtensionEvent<'_>) -> AgentResult<Vec<RuntimeEffect>> {
        let RuntimeExtensionEvent::ToolCompleted { result } = event;
        if result.tool != TODO_TOOL_NAME || !result.ok {
            return Ok(Vec::new());
        }
        let todo = result
            .result
            .as_ref()
            .ok_or_else(|| AgentError::new("todo_update 成功结果缺少 todo 状态。"))
            .and_then(|value| {
                serde_json::from_value::<AgentTodoState>(value.clone()).map_err(|error| {
                    AgentError::new(format!("todo_update 返回了无效的 todo 状态：{error}"))
                })
            })?;

        Ok(vec![RuntimeEffect::EmitEvent(Box::new(
            AgentEvent::TodoUpdated {
                run_id: self.run_id.clone(),
                todo,
            },
        ))])
    }

    fn snapshot_state(&self) -> AgentResult<Value> {
        serde_json::to_value(&*self.state.lock()).map_err(|error| {
            AgentError::new(format!("无法保存 `{TODO_EXTENSION_ID}` 扩展状态：{error}"))
        })
    }

    fn restore_state(&mut self, version: u32, state: Value) -> AgentResult<()> {
        if version != TODO_EXTENSION_VERSION {
            return Err(AgentError::new(format!(
                "无法恢复 `{TODO_EXTENSION_ID}` 扩展状态：不支持快照版本 {version}，当前版本为 {TODO_EXTENSION_VERSION}。"
            )));
        }
        let mut restored = serde_json::from_value::<TodoStateStore>(state).map_err(|error| {
            AgentError::new(format!("无法恢复 `{TODO_EXTENSION_ID}` 扩展状态：{error}"))
        })?;
        restored.validate_and_normalize()?;
        *self.state.lock() = restored;
        Ok(())
    }
}

impl TodoStateHandle {
    pub(super) fn state(&self) -> AgentTodoState {
        self.lock().state.clone()
    }

    fn lock(&self) -> MutexGuard<'_, TodoStateStore> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl TodoStateStore {
    fn new() -> Self {
        Self {
            state: AgentTodoState {
                revision: 0,
                items: Vec::new(),
                updated_at: now_ms(),
            },
            next_item_id: 1,
        }
    }

    fn update(&mut self, args: Value) -> AgentResult<AgentTodoState> {
        let args: TodoUpdateArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("todo_update invalid args: {error}")))?;
        if args
            ._explanation
            .as_deref()
            .is_some_and(|value| value.chars().count() > MAX_TODO_EXPLANATION_CHARS)
        {
            return Err(AgentError::new(format!(
                "todo_update explanation cannot exceed {MAX_TODO_EXPLANATION_CHARS} characters."
            )));
        }
        if args.items.len() > MAX_TODO_ITEMS {
            return Err(AgentError::new(format!(
                "todo_update accepts at most {MAX_TODO_ITEMS} items."
            )));
        }
        if let Some(expected) = args.expected_revision {
            if expected != self.state.revision {
                return Err(AgentError::new(format!(
                    "todo_update expectedRevision {expected} is stale; current revision is {}. Use the latest Runtime todo refs.",
                    self.state.revision
                )));
            }
        }

        let previous = self
            .state
            .items
            .iter()
            .map(|item| (item.id.clone(), item.clone()))
            .collect::<BTreeMap<_, _>>();
        let now = now_ms();
        let mut used_ids = BTreeSet::new();
        let mut next_items = Vec::with_capacity(args.items.len());
        let mut next_item_id = self.next_item_id;

        for item in args.items {
            let referenced = match item.item_ref {
                Some(_) if item.id.is_some() => {
                    return Err(AgentError::new(
                        "todo_update item ref and id are mutually exclusive.",
                    ));
                }
                Some(_) if args.expected_revision.is_none() => {
                    return Err(AgentError::new(
                        "todo_update ref requires expectedRevision from the latest Runtime todo.",
                    ));
                }
                Some(reference) => Some(
                    reference
                        .checked_sub(1)
                        .and_then(|index| self.state.items.get(index))
                        .ok_or_else(|| {
                            AgentError::new(format!(
                                "todo_update item ref {reference} is out of range."
                            ))
                        })?,
                ),
                None => None,
            };
            let title = normalize_required_text(
                item.title
                    .as_deref()
                    .or_else(|| referenced.map(|item| item.title.as_str()))
                    .unwrap_or_default(),
                "todo_update.items[].title",
            )?;
            if title.chars().count() > MAX_TODO_TITLE_CHARS {
                return Err(AgentError::new(format!(
                    "todo_update item title cannot exceed {MAX_TODO_TITLE_CHARS} characters."
                )));
            }
            let note = match item.note.as_deref() {
                Some(value) => normalize_optional_text(value),
                None => referenced.and_then(|item| item.note.clone()),
            };
            if note
                .as_deref()
                .is_some_and(|value| value.chars().count() > MAX_TODO_NOTE_CHARS)
            {
                return Err(AgentError::new(format!(
                    "todo_update item note cannot exceed {MAX_TODO_NOTE_CHARS} characters."
                )));
            }
            let id = match referenced
                .map(|item| item.id.clone())
                .or_else(|| item.id.and_then(|id| normalize_optional_text(&id)))
            {
                Some(id) if id.chars().count() <= MAX_TODO_ID_CHARS => id,
                Some(_) => {
                    return Err(AgentError::new(format!(
                        "todo_update item id cannot exceed {MAX_TODO_ID_CHARS} characters."
                    )))
                }
                None => Self::allocate_item_id(&mut next_item_id, &used_ids, &previous)?,
            };
            if !used_ids.insert(id.clone()) {
                return Err(AgentError::new(format!(
                    "todo_update contains duplicate item id: {id}"
                )));
            }
            let created_at = previous
                .get(&id)
                .map(|existing| existing.created_at)
                .unwrap_or(now);
            next_items.push(AgentTodoItem {
                id: id.clone(),
                title,
                status: item.status,
                note,
                created_at,
                updated_at: now,
            });
        }

        let next_state = AgentTodoState {
            revision: self
                .state
                .revision
                .checked_add(1)
                .ok_or_else(|| AgentError::new("todo_update revision exhausted."))?,
            items: next_items,
            updated_at: now,
        };
        self.next_item_id = next_item_id;
        self.state = next_state;
        Ok(self.state.clone())
    }

    fn allocate_item_id(
        next_item_id: &mut u64,
        used_ids: &BTreeSet<String>,
        previous: &BTreeMap<String, AgentTodoItem>,
    ) -> AgentResult<String> {
        loop {
            let id = format!("todo-{next_item_id}");
            *next_item_id = next_item_id
                .checked_add(1)
                .ok_or_else(|| AgentError::new("todo_update item id allocator exhausted."))?;
            if !used_ids.contains(&id) && !previous.contains_key(&id) {
                return Ok(id);
            }
        }
    }

    fn validate_and_normalize(&mut self) -> AgentResult<()> {
        if self.state.items.len() > MAX_TODO_ITEMS {
            return Err(AgentError::new(format!(
                "Cannot restore todo: at most {MAX_TODO_ITEMS} items are allowed."
            )));
        }
        let mut ids = BTreeSet::new();
        let mut inferred_next_id = 1;
        for item in &self.state.items {
            for (field, value, limit) in [
                ("id", item.id.as_str(), MAX_TODO_ID_CHARS),
                ("title", item.title.as_str(), MAX_TODO_TITLE_CHARS),
            ] {
                if value.trim().is_empty() || value.chars().count() > limit {
                    return Err(AgentError::new(format!("Cannot restore todo: {field} must be non-empty and at most {limit} characters.")));
                }
            }
            if item
                .note
                .as_deref()
                .is_some_and(|value| value.chars().count() > MAX_TODO_NOTE_CHARS)
            {
                return Err(AgentError::new(format!(
                    "Cannot restore todo: note cannot exceed {MAX_TODO_NOTE_CHARS} characters."
                )));
            }
            if !ids.insert(item.id.as_str()) {
                return Err(AgentError::new(format!(
                    "无法恢复 `{TODO_EXTENSION_ID}` 扩展状态：todo id `{}` 重复。",
                    item.id
                )));
            }
            if let Some(value) = item
                .id
                .strip_prefix("todo-")
                .and_then(|value| value.parse::<u64>().ok())
            {
                inferred_next_id = inferred_next_id.max(value.saturating_add(1));
            }
        }
        self.next_item_id = self.next_item_id.max(inferred_next_id).max(1);
        Ok(())
    }
}

struct TodoTool {
    state: TodoStateHandle,
}

impl AgentTool for TodoTool {
    fn exposure(&self) -> crate::tools::AgentToolExposure {
        crate::tools::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> crate::tools::AgentToolPermissionPolicy {
        crate::tools::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        todo_tool_definition()
    }

    fn execute(&self, _context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        let mut store = self.state.lock();
        store.update(args)?;
        let state = store.state.clone();
        serde_json::to_value(state)
            .map_err(|error| AgentError::new(format!("无法序列化 todo_update 结果：{error}")))
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        let projected = result.result.as_ref().map(|value| {
            let items = value
                .get("items")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            json!({
                "accepted": result.ok,
                "revision": value.get("revision"),
                "itemCount": items.len(),
                "completedCount": items
                    .iter()
                    .filter(|item| item.get("status").and_then(Value::as_str) == Some("completed"))
                    .count()
            })
        });
        crate::tools::model_projection::compact_model_result(result, projected)
    }
}

fn todo_tool_definition() -> AgentToolDefinition {
    AgentToolDefinition {
        name: TODO_TOOL_NAME.to_string(),
        description: "Create or replace the structured todo plan for this logical run only. Use the latest Runtime todo revision and refs to preserve full items when updating progress. It never carries into a later user turn.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "expectedRevision": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Revision from the latest Runtime todo. Required when any item uses ref; stale references are rejected."
                },
                "items": {
                    "type": "array",
                    "description": "Full replacement list in intended order: include every item to keep, including unchanged items. Reference existing items with ref and status; omitted title/note are preserved. New items require title and status. Multiple items may be in_progress in parallel.",
                    "maxItems": MAX_TODO_ITEMS,
                    "items": {
                        "type": "object",
                        "properties": {
                            "ref": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": MAX_TODO_ITEMS,
                                "description": "1-based item reference from Runtime todo at expectedRevision. Mutually exclusive with id. Preserves the original id and omitted title/note, even when the reminder abbreviates them."
                            },
                            "id": {
                                "type": "string",
                                "description": "Optional exact stable id for a full item. Do not combine with ref. Omit for new items.",
                                "maxLength": MAX_TODO_ID_CHARS
                            },
                            "title": {
                                "type": "string",
                                "description": "Short action-oriented task title. Required without ref; omit with ref to preserve the complete title.",
                                "maxLength": MAX_TODO_TITLE_CHARS
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed", "blocked"]
                            },
                            "note": {
                                "type": "string",
                                "description": "Optional brief blocker/progress note, not a findings report. With ref, omit to preserve the full note or use an empty string to clear it. The model reminder may abbreviate long titles and notes; stored fields remain complete.",
                                "maxLength": MAX_TODO_NOTE_CHARS
                            }
                        },
                        "required": ["status"],
                        "additionalProperties": false
                    }
                },
                "explanation": {
                    "type": "string",
                    "description": "Optional short reason for the plan/progress update.",
                    "maxLength": MAX_TODO_EXPLANATION_CHARS
                }
            },
            "required": ["items"],
            "additionalProperties": false
        }),
        safety: AgentToolSafety::ReadOnly,
        requires_workspace: false,
        requires_approval: false,
        approval_mode: AgentToolApprovalMode::Never,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TodoUpdateArgs {
    items: Vec<TodoUpdateItem>,
    expected_revision: Option<u64>,
    #[serde(rename = "explanation")]
    _explanation: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TodoUpdateItem {
    #[serde(rename = "ref")]
    item_ref: Option<usize>,
    id: Option<String>,
    title: Option<String>,
    status: AgentTodoStatus,
    note: Option<String>,
}

fn normalize_required_text(value: &str, field: &str) -> AgentResult<String> {
    normalize_optional_text(value).ok_or_else(|| AgentError::new(format!("{field} is required.")))
}

fn normalize_optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn render_todo_state(state: &AgentTodoState) -> String {
    let budget = crate::context::ContextTextBudget::heuristic(TODO_CONTEXT_MAX_TOKENS);
    let mut titles = state
        .items
        .iter()
        .map(|item| todo_reminder_text(&item.title))
        .collect::<Vec<_>>();
    let normalized_notes = state
        .items
        .iter()
        .map(|item| item.note.as_deref().map(todo_reminder_text))
        .collect::<Vec<_>>();
    let mut notes = normalized_notes
        .iter()
        .map(|note| note.as_deref())
        .collect::<Vec<_>>();
    let full = render_todo_projection(state, &titles, &notes, false);
    if budget.fits(&full) {
        return full;
    }

    // Preserve current blockers/work and the earliest pending tasks first. This only changes
    // the reminder: a revision-bound ref always addresses the complete authoritative item.
    let mut trim_order = (0..state.items.len()).collect::<Vec<_>>();
    trim_order.sort_by_key(|&index| {
        let priority = match state.items[index].status {
            AgentTodoStatus::Completed => 0,
            AgentTodoStatus::Pending => 1,
            AgentTodoStatus::Blocked => 2,
            AgentTodoStatus::InProgress => 3,
        };
        (priority, std::cmp::Reverse(index))
    });
    for &index in &trim_order {
        notes[index] = None;
        let rendered = render_todo_projection(state, &titles, &notes, true);
        if budget.fits(&rendered) {
            return rendered;
        }
    }

    for index in trim_order {
        let title = titles[index].clone();
        let graphemes = title.graphemes(true).collect::<Vec<_>>();
        titles[index] = "…".to_string();
        if !budget.fits(&render_todo_projection(state, &titles, &notes, true)) {
            continue;
        }
        // Keep the longest fitting prefix without cutting a Unicode grapheme. At most 12
        // ref/status lines plus ellipses always fit, even for maximum-length Unicode ids.
        let mut low = 0;
        let mut high = graphemes.len();
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            titles[index] = format!("{}…", graphemes[..middle].concat());
            if budget.fits(&render_todo_projection(state, &titles, &notes, true)) {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        titles[index] = format!("{}…", graphemes[..low].concat());
        return render_todo_projection(state, &titles, &notes, true);
    }
    unreachable!("validated Todo refs and statuses fit the reminder budget")
}

fn todo_reminder_text(value: &str) -> String {
    // A multiline title/note must not create an apparent extra ref/status row. Keep the
    // authoritative text untouched; only the one-line reminder folds whitespace.
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn render_todo_projection(
    state: &AgentTodoState,
    titles: &[String],
    notes: &[Option<&str>],
    abbreviated: bool,
) -> String {
    let items = state
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let mut line = format!(
                "- ref={} [{}] {}",
                index + 1,
                item.status.as_str(),
                titles[index]
            );
            if let Some(note) = notes[index] {
                line.push_str(": ");
                line.push_str(note);
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n");
    let total = state.items.len();
    let completed = state
        .items
        .iter()
        .filter(|item| item.status == AgentTodoStatus::Completed)
        .count();
    let abbreviation_notice = if abbreviated {
        "\nTitles/notes abbreviated; stored items unchanged."
    } else {
        ""
    };
    format!(
        "## Runtime todo\nrevision: {}\nprogress: {}/{} completed\n{}{}",
        state.revision, completed, total, items, abbreviation_notice
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextFrame;
    use crate::protocol::{AgentApprovalStatus, AgentRunContext, AgentToolCall};

    fn todo_call(args: Value) -> AgentToolCall {
        AgentToolCall {
            id: "todo-call-1".to_string(),
            tool: TODO_TOOL_NAME.to_string(),
            args,
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        }
    }

    fn execute(tool: &TodoTool, args: Value) -> crate::protocol::AgentToolResult {
        let context = ToolExecutionContext::from_run_context(None::<&AgentRunContext>);
        let call = todo_call(args.clone());
        match tool.execute(&context, args) {
            Ok(value) => crate::protocol::AgentToolResult {
                exact_archive_file: None,
                call_id: call.id,
                tool: call.tool,
                ok: true,
                result: Some(value),
                error: None,
            },
            Err(error) => crate::protocol::AgentToolResult {
                exact_archive_file: None,
                call_id: call.id,
                tool: call.tool,
                ok: false,
                result: None,
                error: Some(error.to_string()),
            },
        }
    }

    #[test]
    fn todo_tool_updates_state_and_extension_emits_event() {
        let (mut extension, handle) = TodoExtension::new("run-1".to_string());
        let tool = TodoTool {
            state: handle.clone(),
        };
        let result = execute(
            &tool,
            json!({
                "items": [
                    { "title": "Inspect runtime loop", "status": "completed" },
                    { "title": "Add extension framework", "status": "in_progress" }
                ],
                "explanation": "initial plan"
            }),
        );

        assert!(result.ok, "{:?}", result.error);
        let result_state: AgentTodoState =
            serde_json::from_value(result.result.clone().expect("todo result state")).unwrap();
        assert_eq!(result_state.revision, 1);
        assert_eq!(result_state.items.len(), 2);
        assert!(!result_state.items[0].id.is_empty());
        assert_eq!(result_state.items[0].title, "Inspect runtime loop");
        assert_eq!(result_state.items[0].status, AgentTodoStatus::Completed);
        assert!(result_state.items[0].created_at > 0);
        assert!(result_state.items[0].updated_at > 0);
        assert_eq!(handle.state().revision, 1);
        assert_eq!(handle.state().items.len(), 2);
        let model = tool.model_projection(&result);
        let model = model.result.as_ref().expect("compact todo model result");
        assert_eq!(model["accepted"], true);
        assert_eq!(model["revision"], 1);
        assert_eq!(model["itemCount"], 2);
        assert_eq!(model["completedCount"], 1);
        assert!(model.get("items").is_none());
        let renderer = tool.event_projection(&result);
        assert_eq!(
            renderer.result.as_ref().unwrap()["items"][0]["title"],
            "Inspect runtime loop"
        );
        assert_eq!(
            renderer.result.as_ref().unwrap()["items"][0]["createdAt"],
            result_state.items[0].created_at
        );
        let effects = extension
            .on_event(&RuntimeExtensionEvent::ToolCompleted { result: &result })
            .unwrap();
        let [RuntimeEffect::EmitEvent(event)] = effects.as_slice() else {
            panic!("todo_update must emit one renderer event");
        };
        let AgentEvent::TodoUpdated { run_id, todo } = event.as_ref() else {
            panic!("todo_update must emit TodoUpdated");
        };
        assert_eq!(run_id, "run-1");
        assert_eq!(todo.revision, result_state.revision);
        assert_eq!(todo.items[0].id, result_state.items[0].id);
        assert_eq!(todo.items[0].title, result_state.items[0].title);
        assert_eq!(todo.items[0].status, result_state.items[0].status);
        assert_eq!(todo.items[0].created_at, result_state.items[0].created_at);
        assert_eq!(todo.items[0].updated_at, result_state.items[0].updated_at);
    }

    #[test]
    fn todo_allows_multiple_in_progress_and_multi_step_updates() {
        let (_extension, handle) = TodoExtension::new("run-1".to_string());
        let tool = TodoTool {
            state: handle.clone(),
        };
        let initial = execute(
            &tool,
            json!({
                "items": [
                    { "id": "a", "title": "One", "status": "in_progress" },
                    { "id": "b", "title": "Two", "status": "in_progress" }
                ]
            }),
        );
        assert!(initial.ok, "{:?}", initial.error);

        let completed = execute(
            &tool,
            json!({
                "items": [
                    { "id": "a", "title": "One", "status": "completed" },
                    { "id": "b", "title": "Two", "status": "completed" }
                ]
            }),
        );

        assert!(completed.ok, "{:?}", completed.error);
        assert_eq!(handle.state().revision, 2);
        assert!(handle
            .state()
            .items
            .iter()
            .all(|item| item.status == AgentTodoStatus::Completed));
    }

    #[test]
    fn todo_context_is_request_only_and_excluded_from_compaction_requests() {
        let (extension, handle) = TodoExtension::new("run-1".to_string());
        let tool = TodoTool { state: handle };
        assert!(
            execute(
                &tool,
                json!({
                    "items": [{ "id": "existing", "title": "Read files", "status": "pending" }]
                })
            )
            .ok
        );

        let items = extension
            .request_context(&ModelRequestContext::agent_work())
            .unwrap();
        let context = ContextFrame::new(items);
        let messages = context.to_messages();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].content().contains("Runtime todo"));
        assert!(messages[0].content().contains("Read files"));
        let manifest = context.manifest();
        assert_eq!(manifest.entries[0].sources, vec!["runtime_todo"]);
        assert_eq!(manifest.entries[0].scope, "run");
        assert_eq!(manifest.entries[0].retention, "request_only");

        let compaction_items = extension
            .request_context(&ModelRequestContext {
                purpose: ModelRequestPurpose::ContextCompaction,
            })
            .unwrap();
        assert!(compaction_items.is_empty());
    }

    #[test]
    fn approval_checkpoint_restore_preserves_same_logical_run_state_and_id_allocator() {
        let (extension, handle) = TodoExtension::new("run-1".to_string());
        let tool = TodoTool { state: handle };
        assert!(
            execute(
                &tool,
                json!({ "items": [{ "title": "First", "status": "pending" }] })
            )
            .ok
        );
        let snapshot = extension.snapshot_state().unwrap();

        let (mut restored, restored_handle) = TodoExtension::new("run-1".to_string());
        restored
            .restore_state(TODO_EXTENSION_VERSION, snapshot)
            .unwrap();
        let restored_tool = TodoTool {
            state: restored_handle.clone(),
        };
        let first_id = restored_handle.state().items[0].id.clone();
        assert!(
            execute(
                &restored_tool,
                json!({
                    "items": [
                        { "id": first_id, "title": "First", "status": "completed" },
                        { "title": "Second", "status": "pending" }
                    ]
                })
            )
            .ok
        );

        let state = restored_handle.state();
        assert_eq!(state.revision, 2);
        assert_eq!(state.items[1].id, "todo-2");
    }

    #[test]
    fn a_new_run_always_starts_with_an_empty_todo() {
        let (_first_run, first_handle) = TodoExtension::new("run-1".to_string());
        let first_tool = TodoTool {
            state: first_handle.clone(),
        };
        assert!(
            execute(
                &first_tool,
                json!({ "items": [{ "title": "First run only", "status": "pending" }] })
            )
            .ok
        );

        let (second_run, second_handle) = TodoExtension::new("run-2".to_string());
        assert_eq!(first_handle.state().items.len(), 1);
        assert!(second_handle.state().items.is_empty());
        assert!(second_run
            .request_context(&ModelRequestContext::agent_work())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn todo_schema_has_no_cross_turn_evidence_contract() {
        let definition = todo_tool_definition();
        let schema = definition.input_schema.to_string();
        assert!(!schema.contains("evidenceRefs"));
        assert!(definition.description.contains("this logical run only"));
    }

    #[test]
    fn todo_limits_items_without_rejecting_large_valid_plans() {
        let (_extension, handle) = TodoExtension::new("run-1".to_string());
        let tool = TodoTool {
            state: handle.clone(),
        };
        let too_many = (0..=MAX_TODO_ITEMS)
            .map(|index| {
                json!({
                    "title": format!("item-{index}"),
                    "status": "pending"
                })
            })
            .collect::<Vec<_>>();
        let too_many = execute(&tool, json!({ "items": too_many }));
        assert!(!too_many.ok);
        assert!(too_many
            .error
            .as_deref()
            .is_some_and(|error| error.contains("at most 12 items")));

        let oversized = (0..MAX_TODO_ITEMS)
            .map(|index| {
                json!({
                    "title": format!("{index}-{}", "x".repeat(MAX_TODO_TITLE_CHARS - 3)),
                    "status": "pending",
                    "note": "y".repeat(MAX_TODO_NOTE_CHARS)
                })
            })
            .collect::<Vec<_>>();
        let oversized = execute(&tool, json!({ "items": oversized }));
        assert!(oversized.ok, "{:?}", oversized.error);
        assert_eq!(handle.state().items.len(), MAX_TODO_ITEMS);
        assert_eq!(
            handle.state().items[0].note.as_deref(),
            Some("y".repeat(MAX_TODO_NOTE_CHARS).as_str())
        );
        assert!(
            crate::context::ContextTextBudget::heuristic(TODO_CONTEXT_MAX_TOKENS)
                .fits(&render_todo_state(&handle.state()))
        );
    }

    #[test]
    fn todo_rejects_oversized_text_atomically_instead_of_truncating() {
        let (_extension, handle) = TodoExtension::new("run-1".to_string());
        let tool = TodoTool {
            state: handle.clone(),
        };

        let oversized_title = execute(
            &tool,
            json!({
                "items": [{
                    "title": "x".repeat(MAX_TODO_TITLE_CHARS + 1),
                    "status": "pending"
                }]
            }),
        );
        assert!(!oversized_title.ok);
        assert!(oversized_title
            .error
            .as_deref()
            .is_some_and(|error| error.contains("title cannot exceed 120 characters")));
        assert_eq!(handle.state().revision, 0);
        assert!(handle.state().items.is_empty());

        let oversized_note = execute(
            &tool,
            json!({
                "items": [{
                    "title": "Keep this whole title",
                    "status": "pending",
                    "note": "n".repeat(MAX_TODO_NOTE_CHARS + 1)
                }]
            }),
        );
        assert!(!oversized_note.ok);
        assert!(oversized_note
            .error
            .as_deref()
            .is_some_and(|error| error.contains("note cannot exceed 240 characters")));
        assert_eq!(handle.state().revision, 0);
        assert!(handle.state().items.is_empty());

        let accepted = execute(
            &tool,
            json!({
                "items": [{
                    "title": "Preserved exactly",
                    "status": "pending",
                    "note": "Also preserved exactly"
                }]
            }),
        );
        assert!(accepted.ok, "{:?}", accepted.error);
        let state = handle.state();
        assert_eq!(state.revision, 1);
        assert_eq!(state.items[0].id, "todo-1");
        assert_eq!(state.items[0].title, "Preserved exactly");
        assert_eq!(
            state.items[0].note.as_deref(),
            Some("Also preserved exactly")
        );
    }
}
