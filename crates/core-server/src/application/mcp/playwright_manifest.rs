use mycopilot_core::{
    AgentError, AgentResult, AgentToolSafety, BuiltinCapabilityDescriptor, BuiltinCapabilityId,
    BuiltinCapabilityManifest, BuiltinCapabilityProviderContract, BuiltinCapabilityToolDescriptor,
    BuiltinMcpToolApprovalMode, BuiltinMcpToolRiskKind,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const PLAYWRIGHT_MCP_PACKAGE_NAME: &str = "@playwright/mcp";
pub(crate) const PLAYWRIGHT_MCP_PACKAGE_VERSION: &str = "0.0.79";
pub(crate) const PLAYWRIGHT_RUNTIME_VERSION: &str = "1.63.0-alpha-2026-08-05";
pub(crate) const BROWSER_AUTOMATION_CAPABILITY_ID: &str = "browser_automation";
pub(crate) const BROWSER_AUTOMATION_MANAGED_MCP_ID: &str = "builtin.browser_automation.mcp";
pub(crate) const BROWSER_AUTOMATION_MANAGED_SERVER_ID: &str =
    "b77b3d54-b7c6-4ead-9cbd-3b9fe50d5311";

const UPSTREAM_CATALOG_JSON: &str =
    include_str!("../../../resources/playwright-upstream-catalog-0.0.79.json");
const REVIEWED_POLICY_JSON: &str =
    include_str!("../../../resources/playwright-browser-manifest-v1.json");
const FIXED_UPSTREAM_TOOL_COUNT: usize = 69;
const MAX_REVIEWED_EXPOSED_TOOLS: usize = 128;
const CALL_REASON_PROPERTY: &str = "call_reason";
const BROWSER_PDF_SAVE_DESCRIPTION: &str = "Save the current page as a managed PDF artifact using native webpage printing. Pages with cross-process embedded frames are currently unsupported. A typed unavailable result explains compatibility limits or a pending print; follow its recovery guidance before retrying.";
const BROWSER_TAKE_SCREENSHOT_DESCRIPTION: &str = "Take a screenshot of the current page. Prefer browser_snapshot and DOM targets for actions. When DOM targets are unavailable, inspect the screenshot with read_image before using coordinate tools; use viewport coordinates from a recent screenshot of the same page, and refresh after navigation, scrolling, resizing, or other page changes. A successful result includes readPath as an image-artifact://sha256/... URI; pass that exact value to read_image.path. Do not guess a workspace path, filename, displayName, or artifactId.";
const BROWSER_FILE_UPLOAD_DESCRIPTION: &str = "Upload files through an open file chooser. paths accepts authorized workspace-relative or absolute file paths and accessible file-input references such as browser-download:<uuid>. File access and target approval are checked before upload. Omitting paths cancels the file chooser; it does not open a native file picker.";
const BROWSER_DROP_DESCRIPTION: &str = "Drop files or MIME data onto an element on the current page. Provide paths, data, or both. paths accepts authorized workspace-relative or absolute file paths and accessible file-input references such as browser-download:<uuid>. File access and target approval are checked before dropping files. Use data for a MIME-data-only drop.";
const BROWSER_CLICK_DESCRIPTION: &str = "Perform a click on the current page. If the click starts a browser download, the result reports download_started with a stable download ID; use browser_wait_for with a short time to observe progress.";
const BROWSER_GET_CONFIG_DESCRIPTION: &str = "Get the managed browser's resolved Host configuration and the current task's path-free download progress.";
const BROWSER_WAIT_FOR_DESCRIPTION: &str = "Wait for text to appear or disappear or for a specified time. The result also reports current-task browser download progress, so use a short time to poll an active download.";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PlaywrightToolIdentity(String);

impl PlaywrightToolIdentity {
    fn parse(value: String) -> AgentResult<Self> {
        if !value.starts_with("browser_")
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(AgentError::new(
                "内置 Playwright Tool identity 不是规范 browser_* 名称。",
            ));
        }
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PlaywrightToolCapability {
    Config,
    Core,
    CoreInput,
    CoreNavigation,
    CoreTabs,
    Network,
    Storage,
    Devtools,
    Vision,
    Pdf,
    Testing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlaywrightToolHandlingMode {
    PassThrough,
    HostAdapted,
    ApprovalRequired,
    ArtifactManaged,
    Sandboxed,
    Unsupported,
}

#[derive(Debug, Clone)]
pub(crate) struct PlaywrightToolContract {
    pub(crate) handling_mode: PlaywrightToolHandlingMode,
    pub(crate) exposed: bool,
    pub(crate) upstream_schema_digest: String,
    pub(crate) host_overlay_digest: String,
    pub(crate) host_schema_digest: String,
    pub(crate) host_input_schema: Value,
    pub(crate) reason_code: String,
    pub(crate) constraints: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PlaywrightBrowserContract {
    pub(crate) manifest: BuiltinCapabilityManifest,
    pub(crate) upstream_catalog_digest: String,
    pub(crate) policy_digest: String,
    pub(crate) tools: BTreeMap<PlaywrightToolIdentity, PlaywrightToolContract>,
}

impl PlaywrightBrowserContract {
    pub(crate) fn tool(&self, raw_name: &str) -> Option<&PlaywrightToolContract> {
        self.tools
            .iter()
            .find_map(|(identity, tool)| (identity.as_str() == raw_name).then_some(tool))
    }
}

impl PlaywrightToolContract {
    pub(crate) fn is_runtime_exposable(&self) -> bool {
        self.exposed
            && matches!(
                self.handling_mode,
                PlaywrightToolHandlingMode::PassThrough
                    | PlaywrightToolHandlingMode::HostAdapted
                    | PlaywrightToolHandlingMode::ApprovalRequired
                    | PlaywrightToolHandlingMode::ArtifactManaged
            )
            && !self.reason_code.is_empty()
            && self
                .constraints
                .iter()
                .any(|constraint| constraint == "requires_call_reason")
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LockedUpstreamCatalog {
    schema_version: u32,
    package_name: String,
    package_version: String,
    playwright_version: String,
    capabilities: Vec<String>,
    tools: Vec<LockedUpstreamTool>,
    catalog_digest: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LockedUpstreamTool {
    raw_name: String,
    capability: PlaywrightToolCapability,
    description: String,
    input_schema: Value,
    annotations: LockedToolAnnotations,
    schema_digest: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LockedToolAnnotations {
    title: String,
    read_only_hint: bool,
    destructive_hint: bool,
    open_world_hint: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewedPolicyManifest {
    schema_version: u32,
    package_name: String,
    package_version: String,
    managed_mcp_id: String,
    capability_id: String,
    manifest_version: String,
    upstream_catalog_digest: String,
    exposed_tool_count: usize,
    tools: Vec<ReviewedToolPolicy>,
    policy_digest: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewedToolPolicy {
    raw_name: String,
    model_name: String,
    handling_mode: PlaywrightToolHandlingMode,
    exposed: bool,
    upstream_schema_digest: String,
    host_overlay: HostSchemaOverlay,
    host_overlay_digest: String,
    host_input_schema_digest: String,
    reason_code: String,
    constraints: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HostSchemaOverlay {
    add_call_reason: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    remove_properties: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    property_overrides: BTreeMap<String, Value>,
}

pub(crate) fn load_playwright_browser_manifest() -> AgentResult<BuiltinCapabilityManifest> {
    Ok(load_playwright_browser_contract()?.manifest)
}

pub(crate) fn load_playwright_browser_contract() -> AgentResult<PlaywrightBrowserContract> {
    let upstream: LockedUpstreamCatalog = serde_json::from_str(UPSTREAM_CATALOG_JSON)
        .map_err(|_| AgentError::new("固定 Playwright upstream catalog 无法解析。"))?;
    let reviewed: ReviewedPolicyManifest = serde_json::from_str(REVIEWED_POLICY_JSON)
        .map_err(|_| AgentError::new("内置浏览器 reviewed policy 无法解析。"))?;

    validate_top_level_identity(&upstream, &reviewed)?;
    validate_embedded_digest(
        UPSTREAM_CATALOG_JSON,
        "catalogDigest",
        &upstream.catalog_digest,
        "固定 Playwright upstream catalog digest 不匹配。",
    )?;
    validate_embedded_digest(
        REVIEWED_POLICY_JSON,
        "policyDigest",
        &reviewed.policy_digest,
        "内置浏览器 reviewed policy digest 不匹配。",
    )?;

    let mut upstream_by_name = BTreeMap::new();
    for upstream_tool in upstream.tools {
        let identity = PlaywrightToolIdentity::parse(upstream_tool.raw_name.clone())?;
        validate_upstream_tool(&upstream_tool)?;
        if upstream_by_name.insert(identity, upstream_tool).is_some() {
            return Err(AgentError::new(
                "固定 Playwright upstream catalog 包含重复 Tool。",
            ));
        }
    }
    if upstream_by_name.len() != FIXED_UPSTREAM_TOOL_COUNT {
        return Err(AgentError::new(
            "固定 Playwright upstream catalog 必须精确包含 69 个 Tool。",
        ));
    }

    let mut policy_names = BTreeSet::new();
    let mut contracts = BTreeMap::new();
    let mut exposed_descriptors = Vec::new();
    let mut handling_counts = BTreeMap::<PlaywrightToolHandlingMode, usize>::new();
    for policy in reviewed.tools {
        let identity = PlaywrightToolIdentity::parse(policy.raw_name.clone())?;
        if !policy_names.insert(identity.clone()) {
            return Err(AgentError::new(
                "内置浏览器 reviewed policy 包含重复 Tool。",
            ));
        }
        if policy.model_name != identity.as_str() {
            return Err(AgentError::new(
                "内置浏览器 raw/model Tool identity 不一致。",
            ));
        }
        validate_policy_reason_and_constraints(&policy)?;
        let upstream_tool = upstream_by_name
            .get(&identity)
            .ok_or_else(|| AgentError::new("内置浏览器 policy 引用了未知 upstream Tool。"))?;
        if policy.upstream_schema_digest != upstream_tool.schema_digest {
            return Err(AgentError::new(
                "内置浏览器 policy 的 upstream schema digest 漂移。",
            ));
        }
        if schema_digest(
            &serde_json::to_value(&policy.host_overlay)
                .map_err(|_| AgentError::new("无法序列化内置浏览器 Host schema overlay。"))?,
        )? != policy.host_overlay_digest
        {
            return Err(AgentError::new(
                "内置浏览器 Host schema overlay digest 不匹配。",
            ));
        }
        if policy.exposed
            && matches!(
                policy.handling_mode,
                PlaywrightToolHandlingMode::Sandboxed | PlaywrightToolHandlingMode::Unsupported
            )
        {
            return Err(AgentError::new(
                "需要审批或沙箱的 Playwright Tool 不得提前暴露。",
            ));
        }

        let host_input_schema =
            apply_host_schema_overlay(&upstream_tool.input_schema, &policy.host_overlay)?;
        let host_schema_digest = schema_digest(&host_input_schema)?;
        if host_schema_digest != policy.host_input_schema_digest {
            return Err(AgentError::new(
                "内置浏览器 Host input schema digest 不匹配。",
            ));
        }

        *handling_counts.entry(policy.handling_mode).or_default() += 1;
        if policy.exposed {
            let mut descriptor = BuiltinCapabilityToolDescriptor::new(
                identity.as_str(),
                &policy.model_name,
                match identity.as_str() {
                    "browser_pdf_save" => BROWSER_PDF_SAVE_DESCRIPTION,
                    "browser_take_screenshot" => BROWSER_TAKE_SCREENSHOT_DESCRIPTION,
                    "browser_file_upload" => BROWSER_FILE_UPLOAD_DESCRIPTION,
                    "browser_drop" => BROWSER_DROP_DESCRIPTION,
                    "browser_click" => BROWSER_CLICK_DESCRIPTION,
                    "browser_get_config" => BROWSER_GET_CONFIG_DESCRIPTION,
                    "browser_wait_for" => BROWSER_WAIT_FOR_DESCRIPTION,
                    _ => &upstream_tool.description,
                },
                host_input_schema.clone(),
                tool_safety(&upstream_tool.annotations)?,
                false,
            )?
            .with_upstream_contract(
                identity.as_str(),
                &upstream_tool.schema_digest,
                &policy.host_overlay_digest,
            )?;
            if policy.handling_mode == PlaywrightToolHandlingMode::ApprovalRequired {
                let mode = if matches!(identity.as_str(), "browser_drop" | "browser_file_upload") {
                    BuiltinMcpToolApprovalMode::Dynamic
                } else {
                    BuiltinMcpToolApprovalMode::Always
                };
                descriptor = descriptor
                    .with_builtin_approval_policy(mode, approval_risk_kinds(identity.as_str())?)?;
            }
            if descriptor.schema_digest != host_schema_digest {
                return Err(AgentError::new(
                    "内置浏览器 Provider schema 链的 digest 不匹配。",
                ));
            }
            exposed_descriptors.push(descriptor);
        }

        let contract = PlaywrightToolContract {
            handling_mode: policy.handling_mode,
            exposed: policy.exposed,
            upstream_schema_digest: upstream_tool.schema_digest.clone(),
            host_overlay_digest: policy.host_overlay_digest,
            host_schema_digest,
            host_input_schema,
            reason_code: policy.reason_code,
            constraints: policy.constraints,
        };
        contracts.insert(identity, contract);
    }

    if policy_names.len() != FIXED_UPSTREAM_TOOL_COUNT
        || policy_names != upstream_by_name.keys().cloned().collect()
        || contracts.len() != FIXED_UPSTREAM_TOOL_COUNT
    {
        return Err(AgentError::new(
            "内置浏览器 reviewed policy 未精确分类全部 69 个 upstream Tool。",
        ));
    }
    validate_classification_counts(&handling_counts)?;
    if reviewed.exposed_tool_count == 0
        || reviewed.exposed_tool_count > MAX_REVIEWED_EXPOSED_TOOLS
        || exposed_descriptors.len() != reviewed.exposed_tool_count
    {
        return Err(AgentError::new(
            "内置浏览器 exposed Tool 数量与 reviewed policy 不一致。",
        ));
    }

    let provider_contract = BuiltinCapabilityProviderContract::new(
        PLAYWRIGHT_MCP_PACKAGE_NAME,
        PLAYWRIGHT_MCP_PACKAGE_VERSION,
        &upstream.catalog_digest,
        &reviewed.policy_digest,
    )?;
    let manifest = BuiltinCapabilityManifest::new_with_provider_contract(
        BuiltinCapabilityDescriptor {
            id: BuiltinCapabilityId::parse(BROWSER_AUTOMATION_CAPABILITY_ID)?,
            display_name: "Browser automation".to_string(),
            description: "Control Captain Who's managed in-app browser for the current task."
                .to_string(),
        },
        reviewed.managed_mcp_id,
        reviewed.manifest_version,
        provider_contract,
        exposed_descriptors,
    )?;

    Ok(PlaywrightBrowserContract {
        manifest,
        upstream_catalog_digest: upstream.catalog_digest,
        policy_digest: reviewed.policy_digest,
        tools: contracts,
    })
}

fn validate_policy_reason_and_constraints(policy: &ReviewedToolPolicy) -> AgentResult<()> {
    let valid_token = |value: &str| {
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    };
    let constraints = policy
        .constraints
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if !valid_token(&policy.reason_code)
        || constraints.len() != policy.constraints.len()
        || constraints.iter().any(|value| !valid_token(value))
        || !constraints.contains("requires_call_reason")
    {
        return Err(AgentError::new(
            "内置 Playwright Tool policy reason/constraints 无效。",
        ));
    }
    let required_constraint = match policy.handling_mode {
        PlaywrightToolHandlingMode::PassThrough => "managed_page_context_only",
        PlaywrightToolHandlingMode::HostAdapted => "host_lifecycle_adapter_required",
        PlaywrightToolHandlingMode::ApprovalRequired => "task_scoped_tool_approval",
        PlaywrightToolHandlingMode::ArtifactManaged => "artifact_handle_only",
        PlaywrightToolHandlingMode::Sandboxed => "isolated_process_required",
        PlaywrightToolHandlingMode::Unsupported => "not_model_visible",
    };
    if !constraints.contains(required_constraint) {
        return Err(AgentError::new(
            "内置 Playwright Tool policy 缺少 handling 约束。",
        ));
    }
    Ok(())
}

fn approval_risk_kinds(raw_name: &str) -> AgentResult<Vec<BuiltinMcpToolRiskKind>> {
    use BuiltinMcpToolRiskKind as Risk;
    let risks = match raw_name {
        "browser_cookie_get" | "browser_cookie_list" => vec![Risk::CookieRead],
        "browser_cookie_clear" | "browser_cookie_delete" | "browser_cookie_set" => {
            vec![Risk::CookieWrite]
        }
        "browser_drop" | "browser_file_upload" => {
            vec![Risk::FileRead, Risk::FileUpload]
        }
        "browser_evaluate" => vec![Risk::PageScriptExecution],
        "browser_localstorage_get" | "browser_localstorage_list" => {
            vec![Risk::LocalStorageRead]
        }
        "browser_localstorage_clear"
        | "browser_localstorage_delete"
        | "browser_localstorage_set" => vec![Risk::LocalStorageWrite],
        "browser_network_request" => vec![Risk::NetworkSensitiveRead],
        "browser_sessionstorage_get" | "browser_sessionstorage_list" => {
            vec![Risk::SessionStorageRead]
        }
        "browser_sessionstorage_clear"
        | "browser_sessionstorage_delete"
        | "browser_sessionstorage_set" => vec![Risk::SessionStorageWrite],
        "browser_set_storage_state" => vec![
            Risk::FileRead,
            Risk::CookieWrite,
            Risk::LocalStorageWrite,
            Risk::StorageStateImport,
        ],
        "browser_storage_state" => vec![
            Risk::FileWrite,
            Risk::CookieRead,
            Risk::LocalStorageRead,
            Risk::StorageStateExport,
        ],
        _ => {
            return Err(AgentError::new(
                "ApprovalRequired Playwright Tool 缺少精确风险分类。",
            ))
        }
    };
    Ok(risks)
}

fn validate_top_level_identity(
    upstream: &LockedUpstreamCatalog,
    reviewed: &ReviewedPolicyManifest,
) -> AgentResult<()> {
    let expected_capabilities = BTreeSet::from([
        "config", "core", "network", "pdf", "storage", "testing", "vision", "devtools",
    ]);
    let capabilities = upstream
        .capabilities
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if upstream.schema_version != 1
        || upstream.package_name != PLAYWRIGHT_MCP_PACKAGE_NAME
        || upstream.package_version != PLAYWRIGHT_MCP_PACKAGE_VERSION
        || upstream.playwright_version != PLAYWRIGHT_RUNTIME_VERSION
        || upstream.tools.len() != FIXED_UPSTREAM_TOOL_COUNT
        || capabilities != expected_capabilities
        || reviewed.schema_version != 2
        || reviewed.package_name != PLAYWRIGHT_MCP_PACKAGE_NAME
        || reviewed.package_version != PLAYWRIGHT_MCP_PACKAGE_VERSION
        || reviewed.managed_mcp_id != BROWSER_AUTOMATION_MANAGED_MCP_ID
        || reviewed.capability_id != BROWSER_AUTOMATION_CAPABILITY_ID
        || reviewed.upstream_catalog_digest != upstream.catalog_digest
        || reviewed.tools.len() != FIXED_UPSTREAM_TOOL_COUNT
        || reviewed.exposed_tool_count > MAX_REVIEWED_EXPOSED_TOOLS
    {
        return Err(AgentError::new(
            "内置 Playwright catalog/policy identity 无效。",
        ));
    }
    Ok(())
}

fn validate_upstream_tool(tool: &LockedUpstreamTool) -> AgentResult<()> {
    // The exact 0.0.79 catalog emits one and only one of these hints for all 69 locked tools.
    // This is a version-lock assertion, not a claim about arbitrary future MCP annotations.
    if tool.description.trim().is_empty()
        || tool.annotations.title.trim().is_empty()
        || tool.annotations.read_only_hint == tool.annotations.destructive_hint
        || !tool.annotations.open_world_hint
        || schema_digest(&tool.input_schema)? != tool.schema_digest
    {
        return Err(AgentError::new(
            "固定 Playwright upstream Tool contract 无效或已漂移。",
        ));
    }
    let schema = tool
        .input_schema
        .as_object()
        .ok_or_else(|| AgentError::new("固定 Playwright upstream schema 不是 object。"))?;
    if schema.get("type").and_then(Value::as_str) != Some("object")
        || !schema.get("properties").is_some_and(Value::is_object)
        || schema.get("additionalProperties") != Some(&Value::Bool(false))
    {
        return Err(AgentError::new(
            "固定 Playwright upstream schema 顶层形状无效。",
        ));
    }
    Ok(())
}

fn validate_classification_counts(
    counts: &BTreeMap<PlaywrightToolHandlingMode, usize>,
) -> AgentResult<()> {
    let valid = counts.get(&PlaywrightToolHandlingMode::PassThrough) == Some(&25)
        && counts.get(&PlaywrightToolHandlingMode::HostAdapted) == Some(&8)
        && counts.get(&PlaywrightToolHandlingMode::ApprovalRequired) == Some(&21)
        && counts.get(&PlaywrightToolHandlingMode::ArtifactManaged) == Some(&7)
        && counts.get(&PlaywrightToolHandlingMode::Sandboxed) == Some(&1)
        && counts.get(&PlaywrightToolHandlingMode::Unsupported) == Some(&7)
        && counts.values().sum::<usize>() == FIXED_UPSTREAM_TOOL_COUNT;
    if !valid {
        return Err(AgentError::new(
            "内置 Playwright 69-Tool 六类 handling 分类不完整。",
        ));
    }
    Ok(())
}

fn tool_safety(annotations: &LockedToolAnnotations) -> AgentResult<AgentToolSafety> {
    match (annotations.read_only_hint, annotations.destructive_hint) {
        (true, false) => Ok(AgentToolSafety::ReadOnly),
        (false, true) => Ok(AgentToolSafety::Destructive),
        _ => Err(AgentError::new(
            "固定 Playwright Tool safety annotations 含糊。",
        )),
    }
}

fn apply_host_schema_overlay(
    upstream_schema: &Value,
    overlay: &HostSchemaOverlay,
) -> AgentResult<Value> {
    if !overlay.add_call_reason {
        return Err(AgentError::new(
            "内置 Playwright Host overlay 必须统一添加 call_reason。",
        ));
    }
    let mut host_schema = upstream_schema.clone();
    let host_object = host_schema
        .as_object_mut()
        .ok_or_else(|| AgentError::new("内置 Playwright upstream schema 不是 object。"))?;
    let mut required = host_object
        .get("required")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let properties = host_object
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| AgentError::new("内置 Playwright upstream properties 缺失。"))?;

    let mut removed = BTreeSet::new();
    for property in &overlay.remove_properties {
        if !removed.insert(property.as_str())
            || required
                .iter()
                .any(|value| value.as_str() == Some(property))
            || properties.remove(property).is_none()
        {
            return Err(AgentError::new(
                "Host overlay 只能删除存在且非 required 的 upstream property。",
            ));
        }
    }
    for (property, patch) in &overlay.property_overrides {
        let upstream_property = properties
            .get_mut(property)
            .ok_or_else(|| AgentError::new("Host overlay 只能收窄存在的 upstream property。"))?;
        validate_property_override(upstream_property, patch)?;
        deep_merge(upstream_property, patch);
    }
    if properties.contains_key(CALL_REASON_PROPERTY) {
        return Err(AgentError::new(
            "upstream schema 意外占用了 Host call_reason 字段。",
        ));
    }
    properties.insert(
        CALL_REASON_PROPERTY.to_string(),
        serde_json::json!({
            "type": "string",
            "description": "Brief reason for using this browser tool.",
            "minLength": 1,
            "maxLength": 512
        }),
    );
    required.push(Value::String(CALL_REASON_PROPERTY.to_string()));
    host_object.insert("required".to_string(), Value::Array(required));

    verify_structural_schema_preserved(upstream_schema, &host_schema, &removed)?;
    Ok(host_schema)
}

fn validate_property_override(upstream: &Value, patch: &Value) -> AgentResult<()> {
    let patch = patch.as_object().ok_or_else(|| {
        AgentError::new("Host property override 必须是 description/enum object。")
    })?;
    if patch.is_empty()
        || patch
            .keys()
            .any(|key| !matches!(key.as_str(), "description" | "enum" | "default"))
    {
        return Err(AgentError::new(
            "Host property override 只允许 description、enum 和受约束 default。",
        ));
    }
    if patch.get("description").is_some_and(|description| {
        description
            .as_str()
            .is_none_or(|description| description.trim().is_empty() || description.len() > 4_096)
    }) {
        return Err(AgentError::new("Host property override description 无效。"));
    }
    if let Some(restricted_enum) = patch.get("enum") {
        let upstream_enum = upstream
            .get("enum")
            .and_then(Value::as_array)
            .ok_or_else(|| AgentError::new("Host enum override 没有 upstream enum。"))?;
        let restricted_enum = restricted_enum
            .as_array()
            .filter(|values| !values.is_empty())
            .ok_or_else(|| AgentError::new("Host enum override 不能为空。"))?;
        let has_duplicate = restricted_enum
            .iter()
            .enumerate()
            .any(|(index, value)| restricted_enum[..index].contains(value));
        if has_duplicate
            || restricted_enum
                .iter()
                .any(|value| !upstream_enum.contains(value))
        {
            return Err(AgentError::new(
                "Host enum override 必须是 upstream enum 的非空子集。",
            ));
        }
    }
    if let Some(restricted_default) = patch.get("default") {
        let upstream_default = upstream
            .get("default")
            .ok_or_else(|| AgentError::new("Host default override 没有 upstream default。"))?;
        let allowed = patch
            .get("enum")
            .or_else(|| upstream.get("enum"))
            .and_then(Value::as_array)
            .is_some_and(|values| values.contains(restricted_default));
        if restricted_default == upstream_default || !allowed {
            return Err(AgentError::new(
                "Host default override 必须改变值且属于最终 enum。",
            ));
        }
    }
    Ok(())
}

fn verify_structural_schema_preserved(
    upstream_schema: &Value,
    host_schema: &Value,
    removed: &BTreeSet<&str>,
) -> AgentResult<()> {
    let upstream = upstream_schema
        .as_object()
        .ok_or_else(|| AgentError::new("upstream schema 结构无效。"))?;
    let host = host_schema
        .as_object()
        .ok_or_else(|| AgentError::new("Host schema 结构无效。"))?;
    let upstream_properties = upstream["properties"]
        .as_object()
        .ok_or_else(|| AgentError::new("upstream properties 结构无效。"))?;
    let host_properties = host["properties"]
        .as_object()
        .ok_or_else(|| AgentError::new("Host properties 结构无效。"))?;
    for (name, upstream_property) in upstream_properties {
        if removed.contains(name.as_str()) {
            continue;
        }
        let host_property = host_properties
            .get(name)
            .ok_or_else(|| AgentError::new("Host overlay 丢失了 upstream property。"))?;
        verify_nested_structural_keywords(upstream_property, host_property)?;
    }
    let upstream_required = upstream
        .get("required")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let host_required = host
        .get("required")
        .and_then(Value::as_array)
        .ok_or_else(|| AgentError::new("Host schema required 缺失。"))?;
    if upstream_required
        .iter()
        .any(|required| !host_required.contains(required))
        || !host_required.contains(&Value::String(CALL_REASON_PROPERTY.to_string()))
    {
        return Err(AgentError::new(
            "Host overlay 丢失了 upstream required 字段。",
        ));
    }
    Ok(())
}

fn verify_nested_structural_keywords(upstream: &Value, host: &Value) -> AgentResult<()> {
    const STRUCTURAL_KEYS: &[&str] = &[
        "type",
        "properties",
        "items",
        "required",
        "additionalProperties",
        "propertyNames",
        "allOf",
        "anyOf",
        "oneOf",
        "$ref",
    ];
    let (Some(upstream), Some(host)) = (upstream.as_object(), host.as_object()) else {
        return Ok(());
    };
    for key in STRUCTURAL_KEYS {
        if upstream.get(*key) != host.get(*key) {
            return Err(AgentError::new(
                "Host overlay 改写或丢失了 upstream required/items/properties。",
            ));
        }
    }
    Ok(())
}

fn deep_merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(base), Value::Object(patch)) => {
            for (key, value) in patch {
                if let Some(existing) = base.get_mut(key) {
                    deep_merge(existing, value);
                } else {
                    base.insert(key.clone(), value.clone());
                }
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

fn validate_embedded_digest(
    source: &str,
    digest_field: &str,
    expected: &str,
    message: &str,
) -> AgentResult<()> {
    let mut value: Value =
        serde_json::from_str(source).map_err(|_| AgentError::new(message.to_string()))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| AgentError::new(message.to_string()))?;
    let embedded = object
        .remove(digest_field)
        .and_then(|value| value.as_str().map(str::to_string))
        .ok_or_else(|| AgentError::new(message.to_string()))?;
    if embedded != expected || schema_digest(&value)? != expected {
        return Err(AgentError::new(message.to_string()));
    }
    Ok(())
}

fn schema_digest(schema: &Value) -> AgentResult<String> {
    let canonical = canonical_json(schema);
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|error| AgentError::new(format!("无法计算 Playwright schema digest：{error}")))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut canonical = Map::with_capacity(values.len());
            for key in keys {
                canonical.insert(key.clone(), canonical_json(&values[key]));
            }
            Value::Object(canonical)
        }
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_catalog_classifies_all_69_tools_and_exposes_reviewed_61() {
        let contract = load_playwright_browser_contract().unwrap();
        assert_eq!(contract.tools.len(), 69);
        assert_eq!(contract.manifest.tools.len(), 61);
        assert_eq!(
            contract.upstream_catalog_digest,
            "sha256:6c24d29f58242f59fa4e53e46ff5216170a21358d613a8b5f7f5d323c0080fbf"
        );
        assert_eq!(
            contract.policy_digest,
            "sha256:cf4b0d4ba01ce5c695096ad4dca499ceb895fd3c08c6f638b5602c1a21dc6e0c"
        );

        let counts = contract.tools.values().fold(
            BTreeMap::<PlaywrightToolHandlingMode, usize>::new(),
            |mut counts, tool| {
                *counts.entry(tool.handling_mode).or_default() += 1;
                counts
            },
        );
        validate_classification_counts(&counts).unwrap();
    }

    #[test]
    fn fixed_catalog_preserves_official_tool_capability_groups() {
        let upstream: LockedUpstreamCatalog = serde_json::from_str(UPSTREAM_CATALOG_JSON).unwrap();
        let capabilities = upstream
            .tools
            .into_iter()
            .map(|tool| (tool.raw_name, tool.capability))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            capabilities["browser_navigate"],
            PlaywrightToolCapability::CoreNavigation
        );
        assert_eq!(
            capabilities["browser_navigate_back"],
            PlaywrightToolCapability::CoreNavigation
        );
        assert_eq!(
            capabilities["browser_press_key"],
            PlaywrightToolCapability::CoreInput
        );
        assert_eq!(
            capabilities["browser_type"],
            PlaywrightToolCapability::CoreInput
        );
        assert_eq!(
            capabilities["browser_tabs"],
            PlaywrightToolCapability::CoreTabs
        );
    }

    #[test]
    fn provider_manifest_contains_reviewed_pass_through_and_sensitive_tools() {
        let manifest = load_playwright_browser_manifest().unwrap();
        let names = manifest
            .tools
            .iter()
            .map(|tool| tool.model_name.as_str())
            .collect::<BTreeSet<_>>();
        for expected in [
            "browser_drag",
            "browser_handle_dialog",
            "browser_hover",
            "browser_navigate_back",
            "browser_select_option",
            "browser_generate_locator",
            "browser_mouse_click_xy",
            "browser_verify_value",
        ] {
            assert!(names.contains(expected), "{expected}");
        }
        for sensitive in [
            "browser_evaluate",
            "browser_file_upload",
            "browser_cookie_list",
            "browser_network_request",
            "browser_storage_state",
        ] {
            assert!(names.contains(sensitive), "{sensitive}");
        }
        let forbidden = "browser_run_code_unsafe";
        assert!(!names.contains(forbidden), "{forbidden}");
        let screenshot = manifest
            .tools
            .iter()
            .find(|tool| tool.model_name == "browser_take_screenshot")
            .unwrap();
        assert!(screenshot.description.contains("read_image.path"));
        assert!(screenshot
            .description
            .contains("Prefer browser_snapshot and DOM targets"));
        assert!(screenshot
            .description
            .contains("inspect the screenshot with read_image"));
        assert!(screenshot.description.contains("viewport coordinates"));
        assert!(screenshot
            .description
            .contains("refresh after navigation, scrolling, resizing"));
        assert!(screenshot.description.contains("image-artifact://sha256/"));
        assert!(screenshot
            .description
            .contains("Do not guess a workspace path, filename, displayName, or artifactId."));
        for tool_name in ["browser_file_upload", "browser_drop"] {
            let tool = manifest
                .tools
                .iter()
                .find(|tool| tool.model_name == tool_name)
                .unwrap();
            assert!(tool
                .description
                .contains("authorized workspace-relative or absolute file paths"));
            assert!(tool.input_schema["properties"]["paths"]["description"]
                .as_str()
                .unwrap()
                .contains("browser-download:<uuid>"));
            assert_eq!(tool.input_schema["properties"]["paths"]["type"], "array");
            assert_eq!(
                tool.input_schema["properties"]["paths"]["items"]["type"],
                "string"
            );
        }
    }

    #[test]
    fn host_overlay_preserves_upstream_required_items_and_properties() {
        let contract = load_playwright_browser_contract().unwrap();
        let fill = contract.tool("browser_fill_form").unwrap();
        assert_eq!(
            fill.host_input_schema["properties"]["fields"]["items"]["required"],
            serde_json::json!(["target", "name", "type", "value"])
        );
        assert!(
            fill.host_input_schema["properties"]["fields"]["items"]["properties"]
                .as_object()
                .is_some_and(|properties| {
                    ["element", "target", "name", "type", "value"]
                        .iter()
                        .all(|name| properties.contains_key(*name))
                })
        );
        assert_eq!(
            fill.host_input_schema["properties"]["fields"]["items"]["properties"]["type"]["enum"],
            serde_json::json!(["textbox", "checkbox", "radio", "combobox", "slider"])
        );
        assert_eq!(
            fill.host_input_schema["properties"]["fields"]["type"],
            "array"
        );
        assert_eq!(
            fill.host_input_schema["required"],
            serde_json::json!(["fields", "call_reason"])
        );
    }

    #[test]
    fn reviewed_overlays_are_explicit_and_bounded() {
        let contract = load_playwright_browser_contract().unwrap();
        for tool in contract.tools.values().filter(|tool| tool.exposed) {
            let properties = tool.host_input_schema["properties"].as_object().unwrap();
            let required = tool.host_input_schema["required"].as_array().unwrap();
            assert!(properties.contains_key(CALL_REASON_PROPERTY));
            assert!(required.contains(&serde_json::json!(CALL_REASON_PROPERTY)));
            assert!(!properties.contains_key("approval_origin"));
            assert!(!required.contains(&serde_json::json!("approval_origin")));
            assert_eq!(
                schema_digest(&tool.host_input_schema).unwrap(),
                tool.host_schema_digest
            );
            assert!(tool.upstream_schema_digest.starts_with("sha256:"));
            assert!(matches!(
                tool.handling_mode,
                PlaywrightToolHandlingMode::PassThrough
                    | PlaywrightToolHandlingMode::HostAdapted
                    | PlaywrightToolHandlingMode::ApprovalRequired
                    | PlaywrightToolHandlingMode::ArtifactManaged
            ));
            assert!(!tool.reason_code.is_empty());
            assert!(tool
                .constraints
                .iter()
                .any(|value| value == "requires_call_reason"));
        }

        for tool_name in [
            "browser_console_messages",
            "browser_network_requests",
            "browser_snapshot",
        ] {
            let tool = contract.tool(tool_name).unwrap();
            assert!(tool.host_input_schema["properties"]["filename"].is_object());
            assert_eq!(
                tool.host_input_schema["properties"]["filename"]["type"],
                "string"
            );
        }
        let console = contract.tool("browser_console_messages").unwrap();
        assert_eq!(
            console.host_input_schema["properties"]["all"]["type"],
            "boolean"
        );
        assert_eq!(
            console.host_input_schema["properties"]["level"]["enum"],
            serde_json::json!(["error", "warning", "info", "debug"])
        );
        assert_eq!(
            console.host_input_schema["properties"]["level"]["default"],
            "info"
        );
        let tabs = contract.tool("browser_tabs").unwrap();
        assert_eq!(
            tabs.host_input_schema["properties"]["action"]["enum"],
            serde_json::json!(["list", "new", "close", "select"])
        );
        assert!(tabs.host_input_schema["properties"]["index"].is_object());
        assert!(tabs.host_input_schema["properties"]["url"].is_object());
    }

    #[test]
    fn nested_structural_overrides_fail_closed() {
        let upstream = serde_json::json!({
            "type": "object",
            "properties": {
                "fields": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {"value": {"type": "string"}},
                        "required": ["value"]
                    }
                }
            },
            "additionalProperties": false
        });
        let overlay = HostSchemaOverlay {
            add_call_reason: true,
            remove_properties: Vec::new(),
            property_overrides: BTreeMap::from([(
                "fields".to_string(),
                serde_json::json!({"items": {"properties": {}}}),
            )]),
        };
        assert!(apply_host_schema_overlay(&upstream, &overlay).is_err());
    }

    #[test]
    fn host_override_allowlist_rejects_every_non_description_or_enum_keyword() {
        let upstream = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "new", "close", "select"]
                }
            },
            "required": ["action"],
            "additionalProperties": false
        });
        for keyword in [
            "type",
            "const",
            "properties",
            "items",
            "prefixItems",
            "required",
            "additionalProperties",
            "propertyNames",
            "not",
            "if",
            "then",
            "else",
            "allOf",
            "anyOf",
            "oneOf",
            "$ref",
            "minimum",
            "maximum",
            "minLength",
            "maxLength",
        ] {
            let overlay = HostSchemaOverlay {
                add_call_reason: true,
                remove_properties: Vec::new(),
                property_overrides: BTreeMap::from([(
                    "action".to_string(),
                    serde_json::json!({(keyword): true}),
                )]),
            };
            assert!(
                apply_host_schema_overlay(&upstream, &overlay).is_err(),
                "{keyword}"
            );
        }

        for restricted in [
            serde_json::json!([]),
            serde_json::json!(["list", "list"]),
            serde_json::json!(["list", "unknown"]),
        ] {
            let overlay = HostSchemaOverlay {
                add_call_reason: true,
                remove_properties: Vec::new(),
                property_overrides: BTreeMap::from([(
                    "action".to_string(),
                    serde_json::json!({"enum": restricted}),
                )]),
            };
            assert!(apply_host_schema_overlay(&upstream, &overlay).is_err());
        }
    }
}
