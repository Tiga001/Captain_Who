use super::{AgentTool, ToolExecutionContext};
use crate::conversation_trace::canonical_tool_result_for_context;
use crate::protocol::{
    AgentError, AgentResult, AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use crate::skills::{
    SkillResourceTextReadOptions, SkillResourceUri, DEFAULT_SKILL_RESOURCE_TEXT_PAGE_BYTES,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::skills_list_resources::{map_resource_error, resource_error};

pub(super) struct SkillsReadResourceTool;

impl AgentTool for SkillsReadResourceTool {
    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "skills_read_resource".to_string(),
            description: "Read a UTF-8 text resource from a currently activated Skill by exact revision-bound skill:// URI. Use nextStartByte for lossless progressive disclosure. Binary assets and templates must be materialized instead of returned as base64.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "uri": { "type": "string", "description": "Exact resource URI returned by skills_list_resources." },
                    "startByte": { "type": "integer", "minimum": 0 },
                    "maxBytes": { "type": "integer", "minimum": 4, "maximum": 262144 }
                },
                "required": ["uri"]
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        context.check_cancelled()?;
        let args: ReadArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("skills_read_resource 参数无效：{error}")))?;
        let uri_text = args
            .uri
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AgentError::new("skills_read_resource.uri 不能为空。"))?;
        let uri = SkillResourceUri::parse(uri_text).map_err(|error| {
            resource_error(
                error.code().stable_name(),
                error.recovery().stable_name(),
                error.to_string(),
            )
        })?;
        let options = SkillResourceTextReadOptions::new(
            args.start_byte.unwrap_or(0),
            args.max_bytes
                .unwrap_or(DEFAULT_SKILL_RESOURCE_TEXT_PAGE_BYTES),
        )
        .map_err(map_resource_error)?;
        let page = context
            .skill_resources()?
            .read_text(&uri, options)
            .map_err(map_resource_error)?;
        Ok(json!({
            "uri": page.uri().as_str(),
            "startByte": page.offset(),
            "endByteExclusive": page.end_offset(),
            "totalBytes": page.total_bytes(),
            "returnedBytes": page.text().len(),
            "truncated": page.truncated(),
            "nextStartByte": page.next_offset(),
            "content": page.text(),
        }))
    }

    fn trace_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        without_resource_content(result)
    }

    fn event_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        without_resource_content(result)
    }

    fn checkpoint_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        without_resource_content(result)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadArgs {
    uri: Option<String>,
    start_byte: Option<usize>,
    max_bytes: Option<usize>,
}

fn without_resource_content(result: &AgentToolResult) -> AgentToolResult {
    let mut projected = canonical_tool_result_for_context(result);
    if let Some(object) = projected.result.as_mut().and_then(Value::as_object_mut) {
        if object.remove("content").is_some() {
            object.insert("contentOmittedFromHistory".to_string(), json!(true));
        }
    }
    projected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_projection_omits_disclosed_resource_text() {
        let canonical = AgentToolResult {
            call_id: "read-1".to_string(),
            tool: "skills_read_resource".to_string(),
            ok: true,
            result: Some(json!({
                "uri": "skill://package/example/revision/references/guide.md",
                "content": "private run-scoped instructions",
                "startByte": 0,
                "endByteExclusive": 31,
            })),
            error: None,
        };

        let projected = without_resource_content(&canonical);
        let result = projected.result.unwrap();
        assert!(result.get("content").is_none());
        assert_eq!(
            result
                .get("contentOmittedFromHistory")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert!(canonical.result.as_ref().unwrap().get("content").is_some());
    }
}
