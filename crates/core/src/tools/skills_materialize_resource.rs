use super::{
    clean_relative_path, AgentTool, AgentToolPermissionPolicy, FileWriteToolAccess,
    ToolExecutionContext,
};
use crate::protocol::{
    AgentError, AgentProposedAction, AgentResult, AgentSkillMaterializationRequest, AgentToolCall,
    AgentToolDefinition, AgentToolSafety,
};
use crate::skills::{
    SkillPackageUri, SkillResourceKind, SkillResourceListOptions, SkillResourcePath,
    SkillResourceUri, MAX_SKILL_RESOURCE_LIST_PAGE_SIZE,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::skills_list_resources::{map_resource_error, resource_error};

/// Approval-gated bridge from an immutable Skill package to the mutable run
/// workspace. The tool only prepares a logical action; the host owns the
/// filesystem transaction and revalidates the exact revision at execution.
pub(super) struct SkillsMaterializeResourceTool;

impl AgentTool for SkillsMaterializeResourceTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::RequiresCapability(super::ToolCapabilityId::application_owned(
            super::SKILL_RESOURCES_MATERIALIZE_CAPABILITY,
        ))
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "skills_materialize_resource".to_string(),
            description: "Copy one immutable asset, or a templates/ subtree, from a currently activated Skill into a new workspace path. The operation is create-only, revision-bound, atomic, and requires file-write authorization. It never overwrites or merges an existing destination.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "sourceUri": { "type": "string", "description": "For one file, use its exact skill:// resource URI. For a template tree, use the exact skill:// package root URI." },
                    "sourcePrefix": { "type": "string", "description": "Optional templates/... package path. When present, sourceUri must be the package root and the complete subtree is copied." },
                    "destination": { "type": "string", "description": "New workspace-relative file or directory path. Existing destinations are never overwritten or merged." },
                    "reason": { "type": "string", "maxLength": 2000, "description": "Short explanation of why the resource is needed." }
                },
                "required": ["sourceUri", "destination"],
                "additionalProperties": false
            }),
            safety: AgentToolSafety::RequiresApproval,
            requires_workspace: true,
            requires_approval: true,
            approval_mode: crate::protocol::AgentToolApprovalMode::Always,
        }
    }

    fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
        Err(AgentError::new(
            "skills_materialize_resource requires host authorization and cannot write from the agent runtime.",
        ))
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        AgentToolPermissionPolicy::FileWrite(FileWriteToolAccess::WriteOnly)
    }

    fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        let args: MaterializeArgs = serde_json::from_value(call.args.clone()).map_err(|error| {
            AgentError::new(format!(
                "skills_materialize_resource parameters are invalid: {error}"
            ))
        })?;
        let source_uri = required(args.source_uri.as_deref(), "sourceUri")?;
        let destination = required(args.destination.as_deref(), "destination")?;
        let destination = clean_relative_path(destination)?
            .to_string_lossy()
            .replace('\\', "/");
        validate_destination(&destination)?;

        let source_prefix = non_empty(args.source_prefix.as_deref()).map(ToString::to_string);
        match source_prefix.as_deref() {
            None => validate_single_resource(context, source_uri)?,
            Some(prefix) => validate_template_tree(context, source_uri, prefix)?,
        }

        let reason = non_empty(args.reason.as_deref())
            .map(|reason| truncate_chars(reason, 2_000))
            .or_else(|| call.reason.clone());
        Ok(AgentProposedAction::SkillMaterialization {
            materialization: AgentSkillMaterializationRequest {
                id: call.id.clone(),
                source_uri: source_uri.to_string(),
                source_prefix,
                destination,
                approval_status: call.approval_status,
                reason,
            },
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MaterializeArgs {
    source_uri: Option<String>,
    source_prefix: Option<String>,
    destination: Option<String>,
    reason: Option<String>,
}

pub(crate) fn validate_frozen_materialization_trace_args(
    frozen: &AgentSkillMaterializationRequest,
    operation: &Value,
) -> Result<(), String> {
    let args: MaterializeArgs = serde_json::from_value(operation.clone()).map_err(|error| {
        format!("skills_materialize_resource frozen ToolCall arguments are invalid: {error}")
    })?;
    let source_uri = required(args.source_uri.as_deref(), "sourceUri")
        .map_err(|_| "materialization sourceUri is invalid".to_string())?;
    let destination = required(args.destination.as_deref(), "destination")
        .map_err(|_| "materialization destination is invalid".to_string())?;
    let destination = clean_relative_path(destination)
        .map_err(|_| "materialization destination is invalid".to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    validate_destination(&destination)
        .map_err(|_| "materialization destination is denied".to_string())?;
    let source_prefix = non_empty(args.source_prefix.as_deref()).map(ToString::to_string);
    let reason_was_present = args.reason.is_some();
    let reason = non_empty(args.reason.as_deref()).map(|reason| truncate_chars(reason, 2_000));

    if source_uri != frozen.source_uri
        || source_prefix != frozen.source_prefix
        || destination != frozen.destination
        || (reason_was_present && reason != frozen.reason)
    {
        return Err(
            "skills_materialize_resource ToolCall differs from the frozen request".to_string(),
        );
    }
    Ok(())
}

fn validate_single_resource(context: &ToolExecutionContext, source: &str) -> AgentResult<()> {
    let uri = SkillResourceUri::parse(source).map_err(|error| {
        resource_error(
            error.code().stable_name(),
            error.recovery().stable_name(),
            error.to_string(),
        )
    })?;
    let options = SkillResourceListOptions::new(1)
        .map_err(map_resource_error)?
        .with_prefix(uri.path().clone());
    let page = context
        .skill_resources()?
        .list(uri.package(), &options)
        .map_err(map_resource_error)?;
    let descriptor = page
        .entries()
        .first()
        .filter(|entry| entry.descriptor().path() == uri.path().as_str())
        .map(|entry| entry.descriptor())
        .ok_or_else(|| {
            resource_error(
                "resourceNotFound",
                "listResources",
                format!("activated Skill does not expose `{}`", uri.path()),
            )
        })?;
    if descriptor.kind() != SkillResourceKind::Asset && !descriptor.path().starts_with("templates/")
    {
        return Err(materialization_error(
            "sourceKindDenied",
            "changeRequest",
            "Only assets and files below templates/ may be materialized.",
        ));
    }
    Ok(())
}

fn validate_template_tree(
    context: &ToolExecutionContext,
    source_root: &str,
    prefix: &str,
) -> AgentResult<()> {
    let package = SkillPackageUri::parse(source_root).map_err(|error| {
        resource_error(
            error.code().stable_name(),
            error.recovery().stable_name(),
            error.to_string(),
        )
    })?;
    let prefix = SkillResourcePath::parse(prefix.to_string()).map_err(|error| {
        resource_error(
            error.code().stable_name(),
            error.recovery().stable_name(),
            error.to_string(),
        )
    })?;
    if prefix.as_str() != "templates" && !prefix.as_str().starts_with("templates/") {
        return Err(materialization_error(
            "sourceKindDenied",
            "changeRequest",
            "Tree materialization is restricted to templates/.",
        ));
    }
    let descendant_prefix = format!("{}/", prefix.as_str());
    let mut after = None;
    let mut has_descendant = false;
    loop {
        let mut options = SkillResourceListOptions::new(MAX_SKILL_RESOURCE_LIST_PAGE_SIZE)
            .map_err(map_resource_error)?
            .with_prefix(prefix.clone());
        if let Some(cursor) = after.take() {
            options = options.with_after(cursor);
        }
        let page = context
            .skill_resources()?
            .list(&package, &options)
            .map_err(map_resource_error)?;
        if page
            .entries()
            .iter()
            .any(|entry| entry.descriptor().path().starts_with(&descendant_prefix))
        {
            has_descendant = true;
            break;
        }
        after = page.next_after().cloned();
        if after.is_none() {
            break;
        }
    }
    if !has_descendant {
        return Err(materialization_error(
            "resourceNotFound",
            "listResources",
            "The requested template subtree is empty or unavailable.",
        ));
    }
    Ok(())
}

fn validate_destination(destination: &str) -> AgentResult<()> {
    const RESERVED: &[&str] = &[".git", ".hg", ".svn", ".agents"];
    if destination
        .split('/')
        .any(|component| RESERVED.contains(&component.to_ascii_lowercase().as_str()))
    {
        return Err(materialization_error(
            "destinationDenied",
            "changeRequest",
            "The destination contains an application- or repository-reserved directory.",
        ));
    }
    Ok(())
}

fn required<'a>(value: Option<&'a str>, name: &str) -> AgentResult<&'a str> {
    non_empty(value).ok_or_else(|| AgentError::new(format!("{name} cannot be empty.")))
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn materialization_error(code: &str, recovery: &str, message: &str) -> AgentError {
    AgentError::structured(
        format!("skill_materialization.{code}"),
        message,
        json!({
            "type": "skill_materialization",
            "code": code,
            "recovery": recovery,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_reserved_destination_components() {
        let error = validate_destination("output/.git/config").unwrap_err();
        assert_eq!(
            error.code(),
            Some("skill_materialization.destinationDenied")
        );
    }

    #[test]
    fn rejects_unknown_materialization_authority_fields() {
        assert!(serde_json::from_value::<MaterializeArgs>(json!({
            "sourceUri": "skill://package/example/revision/template.xlsx",
            "destination": "template.xlsx",
            "overwrite": true,
        }))
        .is_err());
    }
}
