use mycopilot_core::{
    AgentError, AgentResult, AgentToolSafety, BuiltinCapabilityDescriptor, BuiltinCapabilityId,
    BuiltinCapabilityManifest, BuiltinCapabilityToolDescriptor,
};
use serde::Deserialize;
use serde_json::Value;

pub(crate) const PLAYWRIGHT_MCP_PACKAGE_NAME: &str = "@playwright/mcp";
pub(crate) const PLAYWRIGHT_MCP_PACKAGE_VERSION: &str = "0.0.79";
pub(crate) const BROWSER_AUTOMATION_CAPABILITY_ID: &str = "browser_automation";
pub(crate) const BROWSER_AUTOMATION_MANAGED_MCP_ID: &str = "builtin.browser_automation.mcp";
pub(crate) const BROWSER_AUTOMATION_MANAGED_SERVER_ID: &str =
    "b77b3d54-b7c6-4ead-9cbd-3b9fe50d5311";

const REVIEWED_MANIFEST_JSON: &str =
    include_str!("../../../resources/playwright-browser-manifest-v1.json");
const MAX_REVIEWED_TOOLS: usize = 32;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewedManifest {
    schema_version: u32,
    package_name: String,
    package_version: String,
    managed_mcp_id: String,
    capability_id: String,
    manifest_version: String,
    tools: Vec<ReviewedTool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewedTool {
    raw_name: String,
    model_name: String,
    description: String,
    safety: ReviewedSafety,
    input_schema: Value,
    schema_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReviewedSafety {
    ReadOnly,
    Destructive,
}

pub(crate) fn load_playwright_browser_manifest() -> AgentResult<BuiltinCapabilityManifest> {
    let reviewed: ReviewedManifest = serde_json::from_str(REVIEWED_MANIFEST_JSON)
        .map_err(|_| AgentError::new("内置浏览器 manifest 无法解析。"))?;
    if reviewed.schema_version != 1
        || reviewed.package_name != PLAYWRIGHT_MCP_PACKAGE_NAME
        || reviewed.package_version != PLAYWRIGHT_MCP_PACKAGE_VERSION
        || reviewed.managed_mcp_id != BROWSER_AUTOMATION_MANAGED_MCP_ID
        || reviewed.capability_id != BROWSER_AUTOMATION_CAPABILITY_ID
        || reviewed.tools.is_empty()
        || reviewed.tools.len() > MAX_REVIEWED_TOOLS
    {
        return Err(AgentError::new("内置浏览器 manifest identity 无效。"));
    }

    let mut tools = Vec::with_capacity(reviewed.tools.len());
    for reviewed_tool in reviewed.tools {
        if reviewed_tool.raw_name != reviewed_tool.model_name {
            return Err(AgentError::new(
                "内置浏览器 raw/model Tool identity 不一致。",
            ));
        }
        let tool = BuiltinCapabilityToolDescriptor::new(
            reviewed_tool.raw_name,
            reviewed_tool.model_name,
            reviewed_tool.description,
            reviewed_tool.input_schema,
            match reviewed_tool.safety {
                ReviewedSafety::ReadOnly => AgentToolSafety::ReadOnly,
                ReviewedSafety::Destructive => AgentToolSafety::Destructive,
            },
            false,
        )?;
        if tool.schema_digest != reviewed_tool.schema_digest {
            return Err(AgentError::new(
                "内置浏览器 reviewed schema digest 不匹配。",
            ));
        }
        tools.push(tool);
    }

    BuiltinCapabilityManifest::new(
        BuiltinCapabilityDescriptor {
            id: BuiltinCapabilityId::parse(BROWSER_AUTOMATION_CAPABILITY_ID)?,
            display_name: "Browser automation".to_string(),
            description: "Control MyCopilot's managed in-app browser for the current task."
                .to_string(),
        },
        reviewed.managed_mcp_id,
        reviewed.manifest_version,
        tools,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_reviewed_manifest_is_valid_and_excludes_unsafe_tools() {
        let manifest = load_playwright_browser_manifest().unwrap();
        let names = manifest
            .tools
            .iter()
            .map(|tool| tool.model_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            manifest.descriptor.id.as_str(),
            BROWSER_AUTOMATION_CAPABILITY_ID
        );
        assert_eq!(manifest.managed_mcp_id, BROWSER_AUTOMATION_MANAGED_MCP_ID);
        assert!(manifest.version.contains(PLAYWRIGHT_MCP_PACKAGE_VERSION));
        assert!(names.contains(&"browser_navigate"));
        assert!(names.contains(&"browser_snapshot"));
        assert!(names.contains(&"browser_close"));
        assert!(!names.contains(&"browser_run_code_unsafe"));
        assert!(!names.contains(&"browser_evaluate"));
    }

    #[test]
    fn every_reviewed_tool_requires_host_only_call_reason() {
        let manifest = load_playwright_browser_manifest().unwrap();
        for tool in &manifest.tools {
            let properties = tool.input_schema["properties"].as_object().unwrap();
            let required = tool.input_schema["required"].as_array().unwrap();
            assert!(
                properties.contains_key("call_reason"),
                "{}",
                tool.model_name
            );
            assert!(required.iter().any(|value| value == "call_reason"));
            assert_eq!(properties["call_reason"]["maxLength"], 512);
        }
    }

    #[test]
    fn reviewed_snapshot_and_tabs_schemas_keep_host_safety_constraints() {
        let manifest = load_playwright_browser_manifest().unwrap();
        let snapshot = manifest
            .tools
            .iter()
            .find(|tool| tool.model_name == "browser_snapshot")
            .unwrap();
        assert!(snapshot.input_schema["properties"]["filename"].is_null());

        let tabs = manifest
            .tools
            .iter()
            .find(|tool| tool.model_name == "browser_tabs")
            .unwrap();
        assert_eq!(
            tabs.input_schema["properties"]["action"]["enum"],
            serde_json::json!(["list"])
        );
    }
}
