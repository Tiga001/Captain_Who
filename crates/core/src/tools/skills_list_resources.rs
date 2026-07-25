use super::{AgentTool, ToolExecutionContext};
use crate::protocol::{AgentError, AgentResult, AgentToolDefinition, AgentToolSafety};
use crate::skills::{
    SkillPackageUri, SkillResourceError, SkillResourceKind, SkillResourceListOptions,
    SkillResourcePath, DEFAULT_SKILL_RESOURCE_LIST_PAGE_SIZE,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub(super) struct SkillsListResourcesTool;

impl AgentTool for SkillsListResourcesTool {
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
            name: "skills_list_resources".to_string(),
            description: "List revision-bound resources exposed by one currently activated Skill. Pass the exact skill:// package root from the activated Skill metadata. Results contain logical URIs and integrity metadata, never managed-store paths or resource bytes.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rootUri": { "type": "string", "description": "Exact skill://package/.../ root URI from activated Skill metadata." },
                    "prefix": { "type": "string", "description": "Optional canonical resource path prefix such as references or templates/report." },
                    "kind": { "type": "string", "enum": ["reference", "asset", "script", "other"] },
                    "afterPath": { "type": "string", "description": "Continuation cursor returned by a previous page." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
                },
                "required": ["rootUri"]
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        context.check_cancelled()?;
        let args: ListArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("skills_list_resources 参数无效：{error}")))?;
        let root_uri = required(&args.root_uri, "rootUri")?;
        let package = SkillPackageUri::parse(root_uri).map_err(|error| {
            resource_error(
                error.code().stable_name(),
                error.recovery().stable_name(),
                error.to_string(),
            )
        })?;
        let mut options = SkillResourceListOptions::new(
            args.limit.unwrap_or(DEFAULT_SKILL_RESOURCE_LIST_PAGE_SIZE),
        )
        .map_err(map_resource_error)?;
        if let Some(prefix) = non_empty(args.prefix.as_deref()) {
            options = options.with_prefix(SkillResourcePath::parse(prefix.to_string()).map_err(
                |error| {
                    resource_error(
                        error.code().stable_name(),
                        error.recovery().stable_name(),
                        error.to_string(),
                    )
                },
            )?);
        }
        if let Some(after) = non_empty(args.after_path.as_deref()) {
            options = options.with_after(SkillResourcePath::parse(after.to_string()).map_err(
                |error| {
                    resource_error(
                        error.code().stable_name(),
                        error.recovery().stable_name(),
                        error.to_string(),
                    )
                },
            )?);
        }
        if let Some(kind) = non_empty(args.kind.as_deref()) {
            options = options.with_kind(parse_kind(kind)?);
        }
        let page = context
            .skill_resources()?
            .list(&package, &options)
            .map_err(map_resource_error)?;
        Ok(json!({
            "rootUri": page.package().as_str(),
            "resources": page.entries().iter().map(|entry| {
                let descriptor = entry.descriptor();
                json!({
                    "uri": entry.uri().as_str(),
                    "path": descriptor.path(),
                    "kind": descriptor.kind().stable_name(),
                    "byteLength": descriptor.byte_length(),
                    "contentDigest": descriptor.content_digest(),
                })
            }).collect::<Vec<_>>(),
            "truncated": page.has_more(),
            "nextAfterPath": page.next_after().map(SkillResourcePath::as_str),
        }))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListArgs {
    root_uri: Option<String>,
    prefix: Option<String>,
    kind: Option<String>,
    after_path: Option<String>,
    limit: Option<usize>,
}

pub(super) fn map_resource_error(error: SkillResourceError) -> AgentError {
    resource_error(
        error.code().stable_name(),
        error.recovery().stable_name(),
        error.message(),
    )
}

pub(super) fn resource_error(code: &str, recovery: &str, message: String) -> AgentError {
    AgentError::structured(
        format!("skill_resource.{code}"),
        message,
        json!({
            "type": "skill_resource",
            "code": code,
            "recovery": recovery,
        }),
    )
}

fn parse_kind(value: &str) -> AgentResult<SkillResourceKind> {
    match value {
        "reference" => Ok(SkillResourceKind::Reference),
        "asset" => Ok(SkillResourceKind::Asset),
        "script" => Ok(SkillResourceKind::Script),
        "other" => Ok(SkillResourceKind::Other),
        _ => Err(AgentError::new(
            "skills_list_resources.kind 必须是 reference、asset、script 或 other。",
        )),
    }
}

fn required<'a>(value: &'a Option<String>, field: &str) -> AgentResult<&'a str> {
    non_empty(value.as_deref())
        .ok_or_else(|| AgentError::new(format!("skills_list_resources.{field} 不能为空。")))
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}
