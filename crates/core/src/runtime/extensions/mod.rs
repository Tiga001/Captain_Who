//! Typed runtime extensions for agent-loop capabilities.
//!
//! Extensions may contribute request-only context, register normal agent tools, react to
//! completed runtime events, and persist versioned state across an approval pause. They do not
//! mutate the loop or emit frontend events directly; the runtime remains the single owner of
//! control flow and applies explicit effects returned by extensions.
//!
//! Per model request, the runtime first clones retained context, then atomically appends extension
//! contributions and runtime-owned guards before sending the provider request. A later compaction
//! extension can build its own request with `ContextCompaction`, replace retained context through
//! the runtime, and restart this preparation phase so transient Todo state is injected exactly
//! once into the rebuilt agent-work request.

mod todo;

use crate::context::{ContextFrame, ContextItem};
use crate::protocol::{
    AgentError, AgentEvent, AgentExtensionSnapshot, AgentResult, AgentTodoState, AgentToolResult,
};
use crate::tools::{AgentTool, ToolRegistry};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use todo::{TodoExtension, TodoStateHandle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ExtensionDescriptor {
    pub(super) id: &'static str,
    pub(super) version: u32,
    pub(super) order: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModelRequestPurpose {
    AgentWork,
    ContextCompaction,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ModelRequestContext {
    pub(super) purpose: ModelRequestPurpose,
}

impl ModelRequestContext {
    pub(super) fn agent_work() -> Self {
        Self {
            purpose: ModelRequestPurpose::AgentWork,
        }
    }
}

pub(super) enum RuntimeExtensionEvent<'a> {
    ToolCompleted { result: &'a AgentToolResult },
}

pub(super) enum RuntimeEffect {
    EmitEvent(AgentEvent),
}

trait RuntimeExtension: Send {
    fn descriptor(&self) -> ExtensionDescriptor;

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        Vec::new()
    }

    fn request_context(&self, _request: &ModelRequestContext) -> AgentResult<Vec<ContextItem>> {
        Ok(Vec::new())
    }

    fn on_event(&mut self, _event: &RuntimeExtensionEvent<'_>) -> AgentResult<Vec<RuntimeEffect>> {
        Ok(Vec::new())
    }

    fn snapshot_state(&self) -> AgentResult<Value>;

    fn restore_state(&mut self, version: u32, state: Value) -> AgentResult<()>;
}

pub(super) struct RuntimeExtensions {
    extensions: Vec<Box<dyn RuntimeExtension>>,
    todo: Option<TodoStateHandle>,
}

impl RuntimeExtensions {
    pub(super) fn for_run(run_id: &str, snapshots: &[AgentExtensionSnapshot]) -> AgentResult<Self> {
        let (todo, todo_handle) = TodoExtension::new(run_id.to_string());
        Self::from_extensions(vec![Box::new(todo)], Some(todo_handle), snapshots)
    }

    fn from_extensions(
        mut extensions: Vec<Box<dyn RuntimeExtension>>,
        todo: Option<TodoStateHandle>,
        snapshots: &[AgentExtensionSnapshot],
    ) -> AgentResult<Self> {
        validate_extensions(&extensions)?;
        extensions.sort_by_key(|extension| {
            let descriptor = extension.descriptor();
            (descriptor.order, descriptor.id)
        });

        let mut snapshots_by_id = BTreeMap::new();
        for snapshot in snapshots {
            if snapshots_by_id
                .insert(snapshot.extension_id.as_str(), snapshot)
                .is_some()
            {
                return Err(AgentError::new(format!(
                    "扩展状态无效：`{}` 出现了重复快照。",
                    snapshot.extension_id
                )));
            }
        }

        for extension in &mut extensions {
            let descriptor = extension.descriptor();
            if let Some(snapshot) = snapshots_by_id.remove(descriptor.id) {
                extension.restore_state(snapshot.version, snapshot.state.clone())?;
            }
        }
        if let Some(unknown_id) = snapshots_by_id.keys().next() {
            return Err(AgentError::new(format!(
                "无法恢复扩展状态：当前运行未注册扩展 `{unknown_id}`。"
            )));
        }

        Ok(Self { extensions, todo })
    }

    pub(super) fn register_tools(&self, registry: &mut ToolRegistry) -> AgentResult<()> {
        for extension in &self.extensions {
            let descriptor = extension.descriptor();
            for tool in extension.tools() {
                registry.register_extension_tool(descriptor.id, tool)?;
            }
        }
        Ok(())
    }

    pub(super) fn contribute_request_context(
        &self,
        request: &ModelRequestContext,
        context: &mut ContextFrame,
    ) -> AgentResult<()> {
        let mut contributions = Vec::new();
        for extension in &self.extensions {
            contributions.extend(extension.request_context(request)?);
        }
        for item in contributions {
            context.push(item);
        }
        Ok(())
    }

    pub(super) fn on_event(
        &mut self,
        event: RuntimeExtensionEvent<'_>,
    ) -> AgentResult<Vec<RuntimeEffect>> {
        let mut effects = Vec::new();
        for extension in &mut self.extensions {
            effects.extend(extension.on_event(&event)?);
        }
        Ok(effects)
    }

    pub(super) fn snapshots(&self) -> AgentResult<Vec<AgentExtensionSnapshot>> {
        self.extensions
            .iter()
            .map(|extension| {
                let descriptor = extension.descriptor();
                Ok(AgentExtensionSnapshot {
                    extension_id: descriptor.id.to_string(),
                    version: descriptor.version,
                    state: extension.snapshot_state()?,
                })
            })
            .collect()
    }

    pub(super) fn todo_state(&self) -> Option<AgentTodoState> {
        self.todo.as_ref().map(TodoStateHandle::state)
    }
}

fn validate_extensions(extensions: &[Box<dyn RuntimeExtension>]) -> AgentResult<()> {
    let mut ids = BTreeSet::new();
    for extension in extensions {
        let descriptor = extension.descriptor();
        if descriptor.id.trim().is_empty() {
            return Err(AgentError::new("扩展注册失败：扩展 id 不能为空。"));
        }
        if !descriptor.id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        }) {
            return Err(AgentError::new(format!(
                "扩展注册失败：`{}` 只能包含小写 ASCII 字母、数字、点、下划线或连字符。",
                descriptor.id
            )));
        }
        if descriptor.version == 0 {
            return Err(AgentError::new(format!(
                "扩展注册失败：`{}` 的版本必须大于 0。",
                descriptor.id
            )));
        }
        if !ids.insert(descriptor.id) {
            return Err(AgentError::new(format!(
                "扩展注册冲突：`{}` 被重复注册。",
                descriptor.id
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentApprovalStatus, AgentProposedAction, AgentToolCall, AgentToolDefinition,
        AgentToolSafety,
    };
    use crate::tools::ToolExecutionContext;
    use serde_json::json;

    struct TestExtension {
        descriptor: ExtensionDescriptor,
        tool_name: Option<&'static str>,
    }

    impl RuntimeExtension for TestExtension {
        fn descriptor(&self) -> ExtensionDescriptor {
            self.descriptor
        }

        fn tools(&self) -> Vec<Box<dyn AgentTool>> {
            self.tool_name
                .map(|name| vec![Box::new(TestTool(name)) as Box<dyn AgentTool>])
                .unwrap_or_default()
        }

        fn snapshot_state(&self) -> AgentResult<Value> {
            Ok(json!({}))
        }

        fn restore_state(&mut self, version: u32, _state: Value) -> AgentResult<()> {
            if version != self.descriptor.version {
                return Err(AgentError::new("unsupported test snapshot"));
            }
            Ok(())
        }
    }

    struct TestTool(&'static str);

    impl AgentTool for TestTool {
        fn definition(&self) -> AgentToolDefinition {
            AgentToolDefinition {
                name: self.0.to_string(),
                description: "test".to_string(),
                input_schema: json!({ "type": "object" }),
                safety: AgentToolSafety::ReadOnly,
                requires_workspace: false,
                requires_approval: false,
                approval_mode: crate::protocol::AgentToolApprovalMode::Never,
            }
        }

        fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
            Ok(json!({}))
        }
    }

    fn descriptor(id: &'static str, order: i32) -> ExtensionDescriptor {
        ExtensionDescriptor {
            id,
            version: 1,
            order,
        }
    }

    #[test]
    fn rejects_duplicate_extension_ids() {
        let result = RuntimeExtensions::from_extensions(
            vec![
                Box::new(TestExtension {
                    descriptor: descriptor("duplicate", 0),
                    tool_name: None,
                }),
                Box::new(TestExtension {
                    descriptor: descriptor("duplicate", 1),
                    tool_name: None,
                }),
            ],
            None,
            &[],
        );

        let error = match result {
            Ok(_) => panic!("duplicate extension ids must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("重复注册"));
    }

    #[test]
    fn extension_tools_cannot_shadow_core_tools() {
        let extensions = RuntimeExtensions::from_extensions(
            vec![Box::new(TestExtension {
                descriptor: descriptor("shadow", 0),
                tool_name: Some("read_file"),
            })],
            None,
            &[],
        )
        .unwrap();
        let mut registry = ToolRegistry::defaults_with_search(None);

        let error = extensions.register_tools(&mut registry).unwrap_err();

        assert!(error.to_string().contains("read_file"));
        assert!(error.to_string().contains("core"));
    }

    #[test]
    fn todo_uses_registry_execution_and_restores_through_manager_snapshot() {
        let mut extensions = RuntimeExtensions::for_run("run-1", &[]).unwrap();
        let mut registry = ToolRegistry::defaults_with_search(None);
        extensions.register_tools(&mut registry).unwrap();
        assert!(registry.definition_for("todo_update").is_some());
        let call = AgentToolCall {
            id: "todo-call".to_string(),
            tool: "todo_update".to_string(),
            args: json!({
                "items": [{ "title": "Persist plan", "status": "in_progress" }]
            }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let result = registry.execute(&ToolExecutionContext::from_run_context(None), &call);
        assert!(result.ok, "{:?}", result.error);
        let effects = extensions
            .on_event(RuntimeExtensionEvent::ToolCompleted { result: &result })
            .unwrap();
        assert_eq!(effects.len(), 1);

        let snapshots = extensions.snapshots().unwrap();
        let restored = RuntimeExtensions::for_run("run-2", &snapshots).unwrap();
        let state = restored.todo_state().unwrap();
        assert_eq!(state.revision, 1);
        assert_eq!(state.items[0].title, "Persist plan");
    }

    #[test]
    fn approval_event_keeps_run_checkpoint_internal() {
        let event = AgentEvent::ApprovalRequired {
            run_id: "run-1".to_string(),
            action: AgentProposedAction::ToolCall {
                call: AgentToolCall {
                    id: "call-1".to_string(),
                    tool: "test".to_string(),
                    args: json!({}),
                    approval_status: AgentApprovalStatus::Required,
                    reason: None,
                },
            },
            checkpoint: crate::protocol::AgentRunCheckpoint {
                version: 1,
                run_id: "run-1".to_string(),
                context_items: Vec::new(),
                next_model_request_index: 1,
                queued_tool_calls: Vec::new(),
                suppressed_narration: false,
                extension_snapshots: vec![AgentExtensionSnapshot {
                    extension_id: "private".to_string(),
                    version: 1,
                    state: json!({ "secret": "internal state" }),
                }],
                pending_tool_call_id: "call-1".to_string(),
            },
        };

        let serialized = serde_json::to_string(&event).unwrap();

        assert!(!serialized.contains("checkpoint"));
        assert!(!serialized.contains("internal state"));
    }
}
