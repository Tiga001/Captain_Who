use super::{
    ExtensionDescriptor, ModelRequestContext, ModelRequestPurpose, RuntimeEffect, RuntimeExtension,
    RuntimeExtensionEvent,
};
use crate::context::{ContextItem, ContextRetention, ContextScope, ContextSource};
use crate::llm::LlmMessageRole;
use crate::protocol::{
    AgentError, AgentEvent, AgentResult, AgentTodoItem, AgentTodoState, AgentTodoStatus,
    AgentToolApprovalMode, AgentToolDefinition, AgentToolSafety,
};
use crate::tools::{AgentTool, ToolExecutionContext};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

const TODO_EXTENSION_ID: &str = "todo";
const TODO_EXTENSION_VERSION: u32 = 1;
const TODO_TOOL_NAME: &str = "todo_update";
const MAX_TODO_ITEMS: usize = 30;
const MAX_TODO_TITLE_CHARS: usize = 240;
const MAX_TODO_NOTE_CHARS: usize = 1_000;

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

        let items = state
            .items
            .iter()
            .map(|item| {
                let mut line = format!("- [{}] {} ({})", item.status.as_str(), item.title, item.id);
                if let Some(note) = item.note.as_deref() {
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
        let completion_instruction = if total > 0 && completed == total {
            "\nAll todo items are completed. The user's planned work appears complete. Do not call more tools unless there is a clear missing requirement or a failed result. Respond to the user with a concise final summary."
        } else {
            ""
        };
        let content = format!(
            "## Runtime todo state\n\
            The host is maintaining this structured todo state only for the current logical run. It never carries into a later user turn. Use `todo_update` when the plan or progress changes. Preserve existing item ids and update all steps whose state has actually changed. Do not claim the todo state changed unless a tool result confirms it.\n\n\
            revision: {}\nprogress: {}/{} completed\n{}{}",
            state.revision, completed, total, items, completion_instruction
        );

        Some(ContextItem::text(
            LlmMessageRole::User,
            content,
            ContextSource::RuntimeExtension,
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
        if args.items.len() > MAX_TODO_ITEMS {
            return Err(AgentError::new(format!(
                "todo_update accepts at most {MAX_TODO_ITEMS} items."
            )));
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

        for item in args.items {
            let title = normalize_required_text(&item.title, "todo_update.items[].title")?;
            let id = match item.id.and_then(|id| normalize_optional_text(&id)) {
                Some(id) => id,
                None => self.allocate_item_id(&used_ids, &previous),
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
                title: truncate_chars(&title, MAX_TODO_TITLE_CHARS),
                status: item.status,
                note: item
                    .note
                    .as_deref()
                    .and_then(normalize_optional_text)
                    .map(|note| truncate_chars(&note, MAX_TODO_NOTE_CHARS)),
                created_at,
                updated_at: now,
            });
        }

        self.state = AgentTodoState {
            revision: self.state.revision + 1,
            items: next_items,
            updated_at: now,
        };
        Ok(self.state.clone())
    }

    fn allocate_item_id(
        &mut self,
        used_ids: &BTreeSet<String>,
        previous: &BTreeMap<String, AgentTodoItem>,
    ) -> String {
        loop {
            let id = format!("todo-{}", self.next_item_id);
            self.next_item_id += 1;
            if !used_ids.contains(&id) && !previous.contains_key(&id) {
                return id;
            }
        }
    }

    fn validate_and_normalize(&mut self) -> AgentResult<()> {
        let mut ids = BTreeSet::new();
        let mut inferred_next_id = 1;
        for item in &self.state.items {
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
}

fn todo_tool_definition() -> AgentToolDefinition {
    AgentToolDefinition {
        name: TODO_TOOL_NAME.to_string(),
        description: "Create or replace the structured todo plan for this logical run only. It never carries into a later user turn.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "description": "Full replacement todo list in intended order. Multiple items may be in_progress when they are being advanced in parallel.",
                    "maxItems": MAX_TODO_ITEMS,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Optional stable id from the previous todo state. Omit for new items."
                            },
                            "title": {
                                "type": "string",
                                "description": "Short action-oriented task title."
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed", "blocked"]
                            },
                            "note": {
                                "type": "string",
                                "description": "Optional compact blocker, evidence, or progress note."
                            }
                        },
                        "required": ["title", "status"]
                    }
                },
                "explanation": {
                    "type": "string",
                    "description": "Optional short reason for the plan/progress update."
                }
            },
            "required": ["items"]
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
    #[serde(rename = "explanation")]
    _explanation: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TodoUpdateItem {
    id: Option<String>,
    title: String,
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

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut output = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        output.push_str("...");
    }
    output
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
                call_id: call.id,
                tool: call.tool,
                ok: true,
                result: Some(value),
                error: None,
            },
            Err(error) => crate::protocol::AgentToolResult {
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
        assert_eq!(handle.state().revision, 1);
        assert_eq!(handle.state().items.len(), 2);
        let effects = extension
            .on_event(&RuntimeExtensionEvent::ToolCompleted { result: &result })
            .unwrap();
        assert!(matches!(
            effects.as_slice(),
            [RuntimeEffect::EmitEvent(event)]
                if matches!(event.as_ref(), AgentEvent::TodoUpdated { run_id, todo } if run_id == "run-1" && todo.revision == 1)
        ));
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
        assert!(messages[0].content.contains("Runtime todo state"));
        assert!(messages[0].content.contains("Read files"));
        let manifest = context.manifest();
        assert_eq!(manifest.entries[0].sources, vec!["runtime_extension"]);
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
}
