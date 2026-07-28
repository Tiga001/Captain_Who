use super::{AgentTool, ToolExecutionContext};
use crate::conversation_trace::canonical_tool_result_for_context;
use crate::llm::LlmMessage;
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
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::RequiresCapability(super::ToolCapabilityId::application_owned(
            super::SKILL_RESOURCES_READ_CAPABILITY,
        ))
    }

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
        fit_resource_page_to_model_budget(
            context,
            page.uri().as_str(),
            page.offset(),
            page.total_bytes(),
            page.text(),
        )
    }

    fn trace_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        without_resource_content(result)
    }

    fn event_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        without_resource_content(result)
    }

    fn checkpoint_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        canonical_tool_result_for_context(result)
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        let projected = resource_model_projection(result.result.as_ref());
        let mut projected = super::model_projection::compact_model_result(result, projected);
        if result.ok {
            let content = result
                .result
                .as_ref()
                .and_then(|value| value.get("content"))
                .and_then(Value::as_str);
            if let (Some(content), Some(output)) = (
                content,
                projected.result.as_mut().and_then(Value::as_object_mut),
            ) {
                // `compact_model_result` intentionally trims presentation strings. Resource
                // content is instead a byte-addressed protocol field, so restore it verbatim
                // after the shared envelope/error cleanup has run.
                output.insert("content".to_string(), Value::String(content.to_string()));
            }
        }
        projected
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadArgs {
    uri: Option<String>,
    start_byte: Option<usize>,
    max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageStopReason {
    EndOfResource,
    MaxBytes,
    OutputBudget,
}

impl PageStopReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::EndOfResource => "end_of_resource",
            Self::MaxBytes => "max_bytes",
            Self::OutputBudget => "output_budget",
        }
    }
}

fn fit_resource_page_to_model_budget(
    context: &ToolExecutionContext,
    uri: &str,
    start_byte: usize,
    total_bytes: usize,
    content: &str,
) -> AgentResult<Value> {
    let initial_reason = if start_byte.saturating_add(content.len()) < total_bytes {
        PageStopReason::MaxBytes
    } else {
        PageStopReason::EndOfResource
    };
    let full = build_resource_page(uri, start_byte, total_bytes, content, initial_reason);
    if resource_page_fits_model_budget(context, &full)? {
        return Ok(full);
    }

    let empty = build_resource_page(
        uri,
        start_byte,
        total_bytes,
        "",
        PageStopReason::OutputBudget,
    );
    if !resource_page_fits_model_budget(context, &empty)? {
        return Err(AgentError::new(
            "skills_read_resource 无法在 10K 模型结果预算内返回资源分页元数据和安全续读游标。",
        ));
    }

    let mut boundaries = content
        .char_indices()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if boundaries.first().copied() != Some(0) {
        boundaries.insert(0, 0);
    }
    boundaries.push(content.len());
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut fitting = 0_usize;
    let mut rejected = boundaries.len().saturating_sub(1);
    while fitting.saturating_add(1) < rejected {
        let candidate = fitting + (rejected - fitting) / 2;
        let page = build_resource_page(
            uri,
            start_byte,
            total_bytes,
            &content[..boundaries[candidate]],
            PageStopReason::OutputBudget,
        );
        if resource_page_fits_model_budget(context, &page)? {
            fitting = candidate;
        } else {
            rejected = candidate;
        }
    }

    if fitting == 0 && !content.is_empty() {
        let first_character_end = boundaries.get(1).copied().unwrap_or(content.len());
        let page = build_resource_page(
            uri,
            start_byte,
            total_bytes,
            &content[..first_character_end],
            PageStopReason::OutputBudget,
        );
        if !resource_page_fits_model_budget(context, &page)? {
            return Err(AgentError::new(
                "skills_read_resource 无法在 10K 模型结果预算内同时返回一个字符和安全续读游标。",
            ));
        }
        return Ok(page);
    }

    Ok(build_resource_page(
        uri,
        start_byte,
        total_bytes,
        &content[..boundaries[fitting]],
        PageStopReason::OutputBudget,
    ))
}

fn build_resource_page(
    uri: &str,
    start_byte: usize,
    total_bytes: usize,
    content: &str,
    stop_reason: PageStopReason,
) -> Value {
    let end_byte = start_byte.saturating_add(content.len());
    let truncated = end_byte < total_bytes;
    json!({
        "uri": uri,
        "startByte": start_byte,
        "endByteExclusive": end_byte,
        "totalBytes": total_bytes,
        "returnedBytes": content.len(),
        "truncated": truncated,
        "truncatedReason": truncated.then(|| stop_reason.as_str()),
        "nextStartByte": truncated.then_some(end_byte),
        "content": content,
    })
}

fn resource_model_projection(source: Option<&Value>) -> Option<Value> {
    let source = source?.as_object()?;
    let source_value = Value::Object(source.clone());
    let mut projected = serde_json::Map::new();
    for field in [
        "uri",
        "startByte",
        "endByteExclusive",
        "totalBytes",
        "truncated",
        "truncatedReason",
        "nextStartByte",
    ] {
        super::model_projection::insert_field(&mut projected, &source_value, field);
    }
    // Resource text is a byte-addressed protocol field, not presentation prose. Preserve it
    // verbatim so leading/trailing whitespace remains covered by the returned byte range.
    if let Some(content) = source.get("content").and_then(Value::as_str) {
        projected.insert("content".to_string(), Value::String(content.to_string()));
    }
    (!projected.is_empty()).then_some(Value::Object(projected))
}

fn resource_page_fits_model_budget(
    context: &ToolExecutionContext,
    source: &Value,
) -> AgentResult<bool> {
    let raw = AgentToolResult {
        exact_archive_file: None,
        call_id: context.tool_call_id()?.to_string(),
        tool: "skills_read_resource".to_string(),
        ok: true,
        result: Some(source.clone()),
        error: None,
    };
    let projected = SkillsReadResourceTool.model_projection(&raw);
    let content = crate::conversation_trace::render_tool_observation(&projected);
    let message = LlmMessage::tool_result(raw.call_id, content, false);
    Ok(context.text_output_budget().estimate_message(&message)
        <= context.text_output_budget().max_tokens())
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
    use crate::context::{
        ContextCapacityDetector, ContextTextBudget, MODEL_TOOL_RESULT_MAX_TOKENS,
    };
    use crate::protocol::{AgentApiStyle, AgentApprovalStatus, AgentToolCall};
    use crate::skills::{
        memory_resource_session_for_test, SkillId, SkillPackageUri, SkillResourceKind,
        SkillResourcePath, SkillRevision, SkillSourceId, MAX_SKILL_RESOURCE_TEXT_PAGE_BYTES,
    };
    use crate::tools::{ToolExecutionContext, ToolRegistry};
    use std::sync::Arc;

    #[test]
    fn history_projection_omits_disclosed_resource_text() {
        let canonical = AgentToolResult {
            exact_archive_file: None,
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

        let checkpoint = SkillsReadResourceTool.checkpoint_projection(&canonical);
        let checkpoint_result = checkpoint.result.as_ref().unwrap();
        assert_eq!(
            checkpoint_result.get("content").and_then(Value::as_str),
            Some("private run-scoped instructions")
        );
        assert!(checkpoint_result.get("contentOmittedFromHistory").is_none());
    }

    #[test]
    fn large_ascii_resource_pages_are_lossless_and_fit_the_complete_model_message() {
        assert_budgeted_pages_are_lossless(
            &format!("  alpha:{}\\value\n", "x".repeat(97)).repeat(2_000),
        );
    }

    #[test]
    fn large_multibyte_resource_pages_are_utf8_aligned_and_lossless() {
        assert_budgeted_pages_are_lossless(&"  天地🙂\\路径\n".repeat(12_000));
    }

    #[test]
    fn explicit_max_bytes_remains_a_utf8_safe_soft_upper_bound() {
        let (context, uri) = resource_fixture("天地".as_bytes().to_vec());
        let registry = ToolRegistry::defaults_with_search(None);
        let call = AgentToolCall {
            id: "skill-resource-soft-bound".to_string(),
            tool: "skills_read_resource".to_string(),
            args: json!({ "uri": uri, "maxBytes": 4 }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let raw = registry.execute(&context, &call);
        assert!(raw.ok, "{:?}", raw.error);
        let page = raw.result.unwrap();
        assert_eq!(page["content"], "天");
        assert_eq!(page["returnedBytes"], 3);
        assert_eq!(page["nextStartByte"], 3);
        assert_eq!(page["truncatedReason"], "max_bytes");
    }

    fn assert_budgeted_pages_are_lossless(expected: &str) {
        let (context, uri) = resource_fixture(expected.as_bytes().to_vec());
        let registry = ToolRegistry::defaults_with_search(None);
        let detector =
            ContextCapacityDetector::for_model("test-model", AgentApiStyle::OpenAiCompatible, &[]);
        let gate = detector.model_tool_result_gate();
        let mut start_byte = 0_usize;
        let mut reconstructed = String::new();
        let mut pages = 0_usize;

        loop {
            let call = AgentToolCall {
                id: format!("skill-resource-page-{pages}"),
                tool: "skills_read_resource".to_string(),
                args: json!({
                    "uri": uri,
                    "startByte": start_byte,
                    "maxBytes": MAX_SKILL_RESOURCE_TEXT_PAGE_BYTES,
                }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            };
            let raw = registry.execute(&context, &call);
            assert!(raw.ok, "{:?}", raw.error);
            assert!(
                !crate::tools::tool_result_truncated_at_source(&raw),
                "a cursor-pageable resource is not truncated at its source"
            );

            let value = raw.result.as_ref().unwrap();
            let returned = value["content"].as_str().unwrap();
            let end_byte = value["endByteExclusive"].as_u64().unwrap() as usize;
            assert_eq!(value["startByte"].as_u64().unwrap() as usize, start_byte);
            assert_eq!(end_byte, start_byte.saturating_add(returned.len()));
            assert!(expected.is_char_boundary(end_byte));
            assert_eq!(
                returned.as_bytes(),
                &expected.as_bytes()[start_byte..end_byte]
            );

            let model = registry.model_projection(&raw);
            assert_eq!(
                model.result.as_ref().unwrap()["content"].as_str(),
                Some(returned),
                "the model projection must preserve byte-addressed whitespace verbatim"
            );
            assert!(
                !gate.would_truncate(&call.id, false, &model),
                "the central 10K gate must not need to shorten an already fitted resource page"
            );

            reconstructed.push_str(returned);
            pages = pages.saturating_add(1);
            assert!(pages < 100, "resource pagination stopped making progress");
            match value["nextStartByte"].as_u64() {
                Some(next) => {
                    assert!(end_byte > start_byte);
                    assert_eq!(next as usize, end_byte);
                    start_byte = end_byte;
                }
                None => break,
            }
        }

        assert!(
            pages > 1,
            "fixture must exercise multiple model-budget pages"
        );
        assert_eq!(reconstructed, expected);
    }

    fn resource_fixture(content: Vec<u8>) -> (ToolExecutionContext, String) {
        let source_id = SkillSourceId::parse("installed:user").unwrap();
        let skill_id =
            SkillId::parse("installed:user:01234567-89ab-4def-8123-456789abcdef").unwrap();
        let revision =
            SkillRevision::parse(format!("skill-package-sha256-v2:{}", "b".repeat(64))).unwrap();
        let package = SkillPackageUri::new(skill_id.clone(), revision.clone());
        let path = SkillResourcePath::parse("references/large.md").unwrap();
        let uri = package.resource(path).as_str().to_string();
        let session = memory_resource_session_for_test(
            skill_id,
            revision,
            source_id,
            vec![(
                "references/large.md".to_string(),
                SkillResourceKind::Reference,
                content,
            )],
        )
        .unwrap();
        let context = ToolExecutionContext::from_run_context(None)
            .with_skill_resources(Some(Arc::new(session)))
            .with_text_output_budget(ContextTextBudget::heuristic(MODEL_TOOL_RESULT_MAX_TOKENS));
        (context, uri)
    }
}
