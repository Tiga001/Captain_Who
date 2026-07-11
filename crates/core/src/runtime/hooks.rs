// Internal runtime hooks for stateful agent extensions.
use super::AgentEventStream;
use crate::context::{ContextFrame, ContextItem, ContextRetention, ContextScope, ContextSource};
use crate::llm::LlmMessageRole;
use crate::protocol::{
    AgentError, AgentEvent, AgentResult, AgentTodoItem, AgentTodoState, AgentTodoStatus,
    AgentToolCall, AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

const TODO_TOOL_NAME: &str = "todo_update";
const MAX_TODO_ITEMS: usize = 30;
const MAX_TODO_TITLE_CHARS: usize = 240;
const MAX_TODO_NOTE_CHARS: usize = 1_000;

pub(super) trait AgentRuntimeHook: Send {
    fn tool_definitions(&self) -> Vec<AgentToolDefinition> {
        Vec::new()
    }

    fn before_llm_request(&mut self, _context: &mut ContextFrame) -> AgentResult<()> {
        Ok(())
    }

    fn handle_tool_call(&mut self, _call: &AgentToolCall) -> Option<AgentResult<AgentToolResult>> {
        None
    }

    fn after_tool_result(
        &mut self,
        _result: &AgentToolResult,
        _event_stream: &mut AgentEventStream,
    ) -> AgentResult<()> {
        Ok(())
    }

    fn before_run_finish(&mut self, _event_stream: &mut AgentEventStream) -> AgentResult<()> {
        Ok(())
    }

    fn todo_state(&self) -> Option<AgentTodoState> {
        None
    }
}

pub(super) struct AgentRuntimeHooks {
    hooks: Vec<Box<dyn AgentRuntimeHook>>,
}

impl AgentRuntimeHooks {
    pub(super) fn for_run(run_id: &str) -> Self {
        let hooks: Vec<Box<dyn AgentRuntimeHook>> =
            vec![Box::new(TodoHook::new(run_id.to_string()))];
        Self { hooks }
    }

    pub(super) fn tool_definitions(&self) -> Vec<AgentToolDefinition> {
        self.hooks
            .iter()
            .flat_map(|hook| hook.tool_definitions())
            .collect()
    }

    pub(super) fn before_llm_request(&mut self, context: &mut ContextFrame) -> AgentResult<()> {
        for hook in &mut self.hooks {
            hook.before_llm_request(context)?;
        }
        Ok(())
    }

    pub(super) fn handle_tool_call(
        &mut self,
        call: &AgentToolCall,
    ) -> Option<AgentResult<AgentToolResult>> {
        for hook in &mut self.hooks {
            if let Some(result) = hook.handle_tool_call(call) {
                return Some(result);
            }
        }
        None
    }

    pub(super) fn after_tool_result(
        &mut self,
        result: &AgentToolResult,
        event_stream: &mut AgentEventStream,
    ) -> AgentResult<()> {
        for hook in &mut self.hooks {
            hook.after_tool_result(result, event_stream)?;
        }
        Ok(())
    }

    pub(super) fn before_run_finish(
        &mut self,
        event_stream: &mut AgentEventStream,
    ) -> AgentResult<()> {
        for hook in &mut self.hooks {
            hook.before_run_finish(event_stream)?;
        }
        Ok(())
    }

    pub(super) fn todo_state(&self) -> Option<AgentTodoState> {
        self.hooks.iter().find_map(|hook| hook.todo_state())
    }
}

struct TodoHook {
    run_id: String,
    state: AgentTodoState,
    next_item_id: u64,
    pending_event: Option<AgentTodoState>,
}

impl TodoHook {
    fn new(run_id: String) -> Self {
        Self {
            run_id,
            state: AgentTodoState {
                revision: 0,
                items: Vec::new(),
                updated_at: now_ms(),
            },
            next_item_id: 1,
            pending_event: None,
        }
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: TODO_TOOL_NAME.to_string(),
            description: "Create or replace the current structured todo plan. Use it when a multi-step task needs tracking or when progress changes.".to_string(),
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
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }

    fn update(&mut self, call: &AgentToolCall) -> AgentToolResult {
        match self.update_state(call.args.clone()) {
            Ok(state) => {
                self.pending_event = Some(state.clone());
                AgentToolResult {
                    call_id: call.id.clone(),
                    tool: call.tool.clone(),
                    ok: true,
                    result: Some(serde_json::to_value(state).unwrap_or_else(|_| json!({}))),
                    error: None,
                }
            }
            Err(error) => AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: None,
                error: Some(error.to_string()),
            },
        }
    }

    fn update_state(&mut self, args: Value) -> AgentResult<AgentTodoState> {
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
                id,
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

    fn todo_snapshot_item(&self) -> Option<ContextItem> {
        if self.state.items.is_empty() {
            return None;
        }

        let items = self
            .state
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
        let total = self.state.items.len();
        let completed = self
            .state
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
            The host is maintaining this structured todo state for the current run. Use `todo_update` when the plan or progress changes. Preserve existing item ids and update all steps whose state has actually changed. Do not claim the todo state changed unless a tool result confirms it.\n\n\
            revision: {}\nprogress: {}/{} completed\n{}{}",
            self.state.revision, completed, total, items, completion_instruction
        );

        Some(ContextItem::text(
            LlmMessageRole::User,
            content,
            ContextSource::RuntimeHook,
            ContextScope::Run,
            ContextRetention::RequestOnly,
        ))
    }
}

impl AgentRuntimeHook for TodoHook {
    fn tool_definitions(&self) -> Vec<AgentToolDefinition> {
        vec![self.definition()]
    }

    fn before_llm_request(&mut self, context: &mut ContextFrame) -> AgentResult<()> {
        let Some(item) = self.todo_snapshot_item() else {
            return Ok(());
        };
        context.push(item);
        Ok(())
    }

    fn handle_tool_call(&mut self, call: &AgentToolCall) -> Option<AgentResult<AgentToolResult>> {
        if call.tool != TODO_TOOL_NAME {
            return None;
        }

        Some(Ok(self.update(call)))
    }

    fn after_tool_result(
        &mut self,
        result: &AgentToolResult,
        event_stream: &mut AgentEventStream,
    ) -> AgentResult<()> {
        if result.tool != TODO_TOOL_NAME || !result.ok {
            return Ok(());
        }
        if let Some(todo) = self.pending_event.take() {
            event_stream.emit(AgentEvent::TodoUpdated {
                run_id: self.run_id.clone(),
                todo,
            });
        }
        Ok(())
    }

    fn todo_state(&self) -> Option<AgentTodoState> {
        Some(self.state.clone())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TodoUpdateArgs {
    items: Vec<TodoUpdateItem>,
    #[allow(dead_code)]
    explanation: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    let mut output = value.chars().take(max_chars).collect::<String>();
    if output.len() < value.len() {
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
    use crate::protocol::AgentApprovalStatus;

    fn todo_call(args: Value) -> AgentToolCall {
        AgentToolCall {
            id: "todo-call-1".to_string(),
            tool: TODO_TOOL_NAME.to_string(),
            args,
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        }
    }

    #[test]
    fn runtime_hooks_always_expose_todo_tool() {
        let hooks = AgentRuntimeHooks::for_run("run-1");
        let tools = hooks
            .tool_definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();

        assert!(tools.contains(&TODO_TOOL_NAME.to_string()));
    }

    #[test]
    fn todo_hook_updates_state_and_emits_event() {
        let mut hook = TodoHook::new("run-1".to_string());
        let result = hook
            .handle_tool_call(&todo_call(json!({
                "items": [
                    { "title": "Inspect runtime loop", "status": "completed" },
                    { "title": "Add todo hook", "status": "in_progress" }
                ],
                "explanation": "initial plan"
            })))
            .unwrap()
            .unwrap();

        assert!(result.ok, "{:?}", result.error);
        let state = hook.todo_state().unwrap();
        assert_eq!(state.revision, 1);
        assert_eq!(state.items.len(), 2);
        assert_eq!(state.items[1].status, AgentTodoStatus::InProgress);

        let mut stream = AgentEventStream::new(None);
        hook.after_tool_result(&result, &mut stream).unwrap();
        assert!(matches!(
            stream.into_events().as_slice(),
            [AgentEvent::TodoUpdated { run_id, todo }]
                if run_id == "run-1" && todo.revision == 1
        ));
    }

    #[test]
    fn todo_hook_allows_multiple_in_progress_items() {
        let mut hook = TodoHook::new("run-1".to_string());
        let result = hook
            .handle_tool_call(&todo_call(json!({
                "items": [
                    { "title": "One", "status": "in_progress" },
                    { "title": "Two", "status": "in_progress" }
                ]
            })))
            .unwrap()
            .unwrap();

        assert!(result.ok, "{:?}", result.error);
        let state = hook.todo_state().unwrap();
        assert_eq!(state.items.len(), 2);
        assert!(state
            .items
            .iter()
            .all(|item| item.status == AgentTodoStatus::InProgress));
    }

    #[test]
    fn todo_hook_allows_multiple_step_changes_after_initial_plan() {
        let mut hook = TodoHook::new("run-1".to_string());
        let initial = hook
            .handle_tool_call(&todo_call(json!({
                "items": [
                    { "id": "a", "title": "One", "status": "pending" },
                    { "id": "b", "title": "Two", "status": "pending" }
                ]
            })))
            .unwrap()
            .unwrap();
        assert!(initial.ok, "{:?}", initial.error);

        let result = hook
            .handle_tool_call(&todo_call(json!({
                "items": [
                    { "id": "a", "title": "One", "status": "completed" },
                    { "id": "b", "title": "Two", "status": "completed" }
                ]
            })))
            .unwrap()
            .unwrap();

        assert!(result.ok, "{:?}", result.error);
        let state = hook.todo_state().unwrap();
        assert_eq!(state.revision, 2);
        assert!(state
            .items
            .iter()
            .all(|item| item.status == AgentTodoStatus::Completed));
    }

    #[test]
    fn todo_snapshot_is_injected_into_request_copy() {
        let mut hook = TodoHook::new("run-1".to_string());
        let _ = hook
            .handle_tool_call(&todo_call(json!({
                "items": [{ "id": "existing", "title": "Read files", "status": "pending" }]
            })))
            .unwrap()
            .unwrap();
        let base_context = ContextFrame::new(vec![ContextItem::text(
            LlmMessageRole::System,
            "system",
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
            ContextRetention::Retained,
        )]);
        let mut context = base_context.clone();

        hook.before_llm_request(&mut context).unwrap();
        let messages = context.to_messages();

        assert_eq!(base_context.to_messages().len(), 1);
        assert_eq!(messages.len(), 2);
        assert!(messages[1].content.contains("Runtime todo state"));
        assert!(messages[1].content.contains("Read files"));
        let manifest = context.manifest();
        assert_eq!(manifest.entries[1].sources, vec!["runtime_hook"]);
        assert_eq!(manifest.entries[1].scope, "run");
        assert_eq!(manifest.entries[1].retention, "request_only");
    }

    #[test]
    fn completed_todo_snapshot_tells_model_to_finalize() {
        let mut hook = TodoHook::new("run-1".to_string());
        let _ = hook
            .handle_tool_call(&todo_call(json!({
                "items": [
                    { "id": "a", "title": "Create report", "status": "completed" }
                ]
            })))
            .unwrap()
            .unwrap();
        let mut context = ContextFrame::default();

        hook.before_llm_request(&mut context).unwrap();
        let messages = context.to_messages();

        assert!(messages[0].content.contains("progress: 1/1 completed"));
        assert!(messages[0].content.contains("All todo items are completed"));
        assert!(messages[0]
            .content
            .contains("Respond to the user with a concise final summary"));
    }
}
