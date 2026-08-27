use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION: u32 = 4;
pub const MANAGED_PLAYWRIGHT_COMMAND_NOTIFICATION_METHOD: &str = "mcp.builtinPlaywright.command";
pub const MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD: &str = "mcp.builtinPlaywright.cancel";
pub const MANAGED_PLAYWRIGHT_COMPLETE_METHOD: &str = "mcp.builtinPlaywright.complete";
pub const MANAGED_PLAYWRIGHT_DISPATCH_PHASE_METHOD: &str = "mcp.builtinPlaywright.dispatchPhase";
pub const BROWSER_RISK_AUTHORIZE_METHOD: &str = "mcp.browserRisk.authorize";
pub const BROWSER_RISK_CANCEL_METHOD: &str = "mcp.browserRisk.cancel";
pub const BROWSER_RISK_PROTOCOL_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedPlaywrightCommandNotification {
    pub schema_version: u32,
    pub request_id: String,
    pub server_id: String,
    pub deadline_ms: u64,
    pub command: ManagedPlaywrightCommand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagedPlaywrightCommand {
    Connect,
    ListTools {
        cursor: Option<String>,
    },
    PrepareSensitiveTool {
        input: Box<ManagedPlaywrightPrepareSensitiveToolInput>,
    },
    ReleaseSensitiveToolBinding {
        #[serde(rename = "bindingId")]
        binding_id: String,
        #[serde(rename = "runId")]
        run_id: String,
        #[serde(rename = "activationId")]
        activation_id: String,
        #[serde(rename = "callId")]
        call_id: String,
        reason: ManagedPlaywrightSensitiveBindingReleaseReason,
    },
    CallTool {
        name: String,
        arguments: Value,
        #[serde(rename = "timeoutMs")]
        timeout_ms: u64,
        #[serde(rename = "authorizationContext")]
        authorization_context: Box<ManagedPlaywrightAuthorizationContext>,
    },
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedPlaywrightAuthorizationContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    pub run_id: String,
    pub capability_id: String,
    pub activation_id: String,
    pub manifest_digest: String,
    pub policy_revision: u64,
    pub grant_expires_at_ms: u64,
    pub invocation_id: String,
    pub call_id: String,
    pub trigger_tool_name: String,
    pub call_reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin_tool_grant: Option<Box<ManagedPlaywrightBuiltinToolGrantContext>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinMcpToolRiskKindDto {
    FileRead,
    FileWrite,
    FileUpload,
    FileDownload,
    CookieRead,
    CookieWrite,
    LocalStorageRead,
    LocalStorageWrite,
    SessionStorageRead,
    SessionStorageWrite,
    StorageStateImport,
    StorageStateExport,
    NetworkSensitiveRead,
    PageScriptExecution,
    UnsafeCodeExecution,
}

/// Post-approval, value-free grant binding revalidated by Main immediately before dispatch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedPlaywrightBuiltinToolGrantContext {
    pub grant_id: String,
    pub approval_id: String,
    pub arguments_digest: String,
    pub resource_scope_digest: String,
    pub target_binding_id: String,
    pub target_binding_digest: String,
    pub origin: Option<String>,
    pub risk_kinds: Vec<BuiltinMcpToolRiskKindDto>,
    pub expires_at_ms: u64,
}

/// Process-only proposal-time request. Main freezes the exact currently selected managed Surface
/// and, when present, file paths already authorized by Core's workspace/attachment resolver.
/// Nothing in this request may enter Renderer, activity, logs, or a checkpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedPlaywrightPrepareSensitiveToolInput {
    pub binding_request_id: String,
    pub binding_scope: ManagedPlaywrightSensitiveBindingScopeDto,
    pub run_id: String,
    pub capability_id: String,
    pub activation_id: String,
    pub manifest_digest: String,
    pub policy_revision: u64,
    pub grant_expires_at_ms: u64,
    pub call_id: String,
    pub tool_name: String,
    pub arguments_digest: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub file_preparation: Option<ManagedPlaywrightSensitiveFilePreparation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedPlaywrightSensitiveBindingScopeDto {
    ManagedSurface,
    ManagedBrowserProfile,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagedPlaywrightSensitiveFilePreparation {
    ResolvedPaths { paths: Vec<String> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedPlaywrightSensitiveBindingReleaseReason {
    ProposalFailed,
    Rejected,
    Cancelled,
    Expired,
    RunRevoked,
    CapabilityRevoked,
    GrantRevoked,
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRiskKindDto {
    InsecureHttp,
    Localhost,
    Loopback,
    PrivateNetwork,
    LinkLocal,
    CloudMetadata,
    NonStandardPort,
    UrlUserinfo,
    DnsPrivateResolution,
    RiskEscalation,
    NewWindow,
    FileUpload,
    FileDownload,
    LocalServiceRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserResolvedAddressClassDto {
    Public,
    Loopback,
    Private,
    LinkLocal,
    CloudMetadata,
    Unresolved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRiskTriggerDto {
    ToolArgument,
    MainFrame,
    Redirect,
    NewWindow,
    Subresource,
    Upload,
    Download,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRiskDispatchCertaintyDto {
    DefinitelyNotDispatched,
    PossiblyDispatched,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserRiskDestinationInput {
    /// Credential-free display URL: normalized origin plus bounded path, without query/fragment.
    pub normalized_url: String,
    pub origin: String,
    pub scheme: String,
    pub ascii_host: String,
    pub effective_port: u16,
    pub address_class: BrowserResolvedAddressClassDto,
    /// Per-process keyed HMAC of the sorted DNS result. Never forwarded to Agent/Renderer.
    pub resolution_fingerprint: String,
    /// Per-process keyed HMAC of the exact target/action identity. Never forwarded to Agent/UI.
    pub target_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserRiskAuthorizeInput {
    pub schema_version: u32,
    pub request_id: String,
    pub parent_request_id: Option<String>,
    pub authorization_context: ManagedPlaywrightAuthorizationContext,
    pub destination: BrowserRiskDestinationInput,
    pub risk_kinds: Vec<BrowserRiskKindDto>,
    pub trigger: BrowserRiskTriggerDto,
    pub dispatch_certainty: BrowserRiskDispatchCertaintyDto,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRiskAuthorizationDecisionDto {
    Approved,
    Rejected,
    Cancelled,
    Expired,
    PolicyDenied,
    UnsupportedHostBoundary,
    OutcomeUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserRiskAuthorizeOutput {
    pub schema_version: u32,
    pub decision: BrowserRiskAuthorizationDecisionDto,
    pub grant_id: Option<String>,
    /// Bounded user feedback or a fixed Host-safe explanation; never a network error body.
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserRiskCancelInput {
    pub schema_version: u32,
    pub request_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserRiskCancelOutput {
    pub schema_version: u32,
    pub accepted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedPlaywrightCancelNotification {
    pub schema_version: u32,
    pub request_id: String,
    pub reason: ManagedPlaywrightCancelReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedPlaywrightCancelReason {
    Cancelled,
    Timeout,
    Shutdown,
}

/// Main-owned monotonic evidence for a single admitted `call_tool` request. Main must obtain an
/// accepted `PossiblyDispatched` acknowledgement before invoking the fixed official handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedPlaywrightDispatchPhase {
    PreDispatch,
    PossiblyDispatched,
    ResponseReceived,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedPlaywrightDispatchPhaseInput {
    pub schema_version: u32,
    pub request_id: String,
    pub phase: ManagedPlaywrightDispatchPhase,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedPlaywrightDispatchPhaseOutput {
    pub schema_version: u32,
    pub accepted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedPlaywrightCompletionInput {
    pub schema_version: u32,
    pub request_id: String,
    pub outcome: ManagedPlaywrightCompletionOutcome,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagedPlaywrightCompletionOutcome {
    Connected {
        protocol: Value,
    },
    ToolsListed {
        page: Value,
    },
    ToolCalled {
        result: Value,
        /// Main/Core-only absolute screenshot file. Never copy this into MCP, Renderer, or model JSON.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "hostImagePublishPath"
        )]
        host_image_publish_path: Option<String>,
    },
    SensitiveToolPrepared {
        #[serde(rename = "bindingId")]
        binding_id: String,
        #[serde(rename = "targetBindingDigest")]
        target_binding_digest: String,
        origin: Option<String>,
        #[serde(rename = "createdAtMs")]
        created_at_ms: u64,
        #[serde(rename = "expiresAtMs")]
        expires_at_ms: u64,
        #[serde(rename = "fileBasenames")]
        file_basenames: Vec<String>,
        #[serde(rename = "fileRevisionDigest")]
        file_revision_digest: Option<String>,
    },
    SensitiveToolBindingReleased {
        released: bool,
    },
    Closed,
    Error {
        code: ManagedPlaywrightBridgeErrorCode,
        #[serde(rename = "dispatchCertainty")]
        dispatch_certainty: ManagedPlaywrightDispatchCertainty,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedPlaywrightBridgeErrorCode {
    Cancelled,
    Timeout,
    Closed,
    Busy,
    SurfaceUnavailable,
    SurfaceCapacityExceeded,
    TargetClosed,
    ToolNotReviewed,
    InvalidArguments,
    CatalogDrift,
    OutputTooLarge,
    ProtocolError,
    OutcomeUnknown,
    InternalSafeError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedPlaywrightDispatchCertainty {
    DefinitelyNotDispatched,
    PossiblyDispatched,
    ResponseReceived,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedPlaywrightCompletionOutput {
    pub schema_version: u32,
    pub accepted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dispatch_phase_wire_is_strict_and_monotonic_by_construction() {
        assert_eq!(MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION, 4);
        let input = ManagedPlaywrightDispatchPhaseInput {
            schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
            request_id: "4d0dd175-0a92-4bd0-8ec4-653058561d04".to_string(),
            phase: ManagedPlaywrightDispatchPhase::PossiblyDispatched,
        };
        let value = serde_json::to_value(&input).unwrap();
        assert_eq!(
            value,
            json!({
                "schemaVersion": 4,
                "requestId": "4d0dd175-0a92-4bd0-8ec4-653058561d04",
                "phase": "possibly_dispatched"
            })
        );
        assert_eq!(
            serde_json::from_value::<ManagedPlaywrightDispatchPhaseInput>(value).unwrap(),
            input
        );
        assert!(
            serde_json::from_value::<ManagedPlaywrightDispatchPhaseInput>(json!({
                "schemaVersion": 4,
                "requestId": "4d0dd175-0a92-4bd0-8ec4-653058561d04",
                "phase": "request_queued"
            }))
            .is_err()
        );
        assert!(
            ManagedPlaywrightDispatchPhase::PreDispatch
                < ManagedPlaywrightDispatchPhase::PossiblyDispatched
        );
        assert!(
            ManagedPlaywrightDispatchPhase::PossiblyDispatched
                < ManagedPlaywrightDispatchPhase::ResponseReceived
        );
    }

    #[test]
    fn call_tool_wire_uses_the_exact_typescript_casing() {
        let authorization_context = ManagedPlaywrightAuthorizationContext {
            conversation_id: Some("conversation_1".to_string()),
            run_id: "run_1".to_string(),
            capability_id: "browser_automation".to_string(),
            activation_id: "42e7ec2d-03f1-49f3-aa0a-e73ca0f88d91".to_string(),
            manifest_digest: format!("sha256:{}", "a".repeat(64)),
            policy_revision: 7,
            grant_expires_at_ms: 1_750_000_900_000,
            invocation_id: "3da0afaf-a475-4da7-b3fc-bca528cc9a5d".to_string(),
            call_id: "call_1".to_string(),
            trigger_tool_name: "browser_snapshot".to_string(),
            call_reason: "Inspect the local fixture.".to_string(),
            builtin_tool_grant: None,
        };
        let value = serde_json::to_value(ManagedPlaywrightCommand::CallTool {
            name: "browser_snapshot".to_string(),
            arguments: json!({"call_reason": "Inspect the local fixture."}),
            timeout_ms: 60_000,
            authorization_context: Box::new(authorization_context.clone()),
        })
        .unwrap();
        assert_eq!(
            value,
            json!({
                "type": "call_tool",
                "name": "browser_snapshot",
                "arguments": {"call_reason": "Inspect the local fixture."},
                "timeoutMs": 60_000,
                "authorizationContext": {
                    "conversationId": "conversation_1",
                    "runId": "run_1",
                    "capabilityId": "browser_automation",
                    "activationId": "42e7ec2d-03f1-49f3-aa0a-e73ca0f88d91",
                    "manifestDigest": format!("sha256:{}", "a".repeat(64)),
                    "policyRevision": 7,
                    "grantExpiresAtMs": 1_750_000_900_000_u64,
                    "invocationId": "3da0afaf-a475-4da7-b3fc-bca528cc9a5d",
                    "callId": "call_1",
                    "triggerToolName": "browser_snapshot",
                    "callReason": "Inspect the local fixture."
                }
            })
        );
        assert_eq!(
            serde_json::from_value::<ManagedPlaywrightCommand>(value).unwrap(),
            ManagedPlaywrightCommand::CallTool {
                name: "browser_snapshot".to_string(),
                arguments: json!({"call_reason": "Inspect the local fixture."}),
                timeout_ms: 60_000,
                authorization_context: Box::new(authorization_context),
            }
        );
    }

    #[test]
    fn browser_risk_wire_is_strict_and_uses_epoch_milliseconds() {
        let input = BrowserRiskAuthorizeInput {
            schema_version: 1,
            request_id: "acb51da4-2716-4b7d-a085-c48864c5a37e".to_string(),
            parent_request_id: None,
            authorization_context: ManagedPlaywrightAuthorizationContext {
                conversation_id: Some("conversation_1".to_string()),
                run_id: "run_1".to_string(),
                capability_id: "browser_automation".to_string(),
                activation_id: "42e7ec2d-03f1-49f3-aa0a-e73ca0f88d91".to_string(),
                manifest_digest: format!("sha256:{}", "a".repeat(64)),
                policy_revision: 7,
                grant_expires_at_ms: 1_750_000_900_000,
                invocation_id: "3da0afaf-a475-4da7-b3fc-bca528cc9a5d".to_string(),
                call_id: "call_1".to_string(),
                trigger_tool_name: "browser_navigate".to_string(),
                call_reason: "Open the local fixture.".to_string(),
                builtin_tool_grant: None,
            },
            destination: BrowserRiskDestinationInput {
                normalized_url: "http://127.0.0.1:8765/fixture".to_string(),
                origin: "http://127.0.0.1:8765".to_string(),
                scheme: "http".to_string(),
                ascii_host: "127.0.0.1".to_string(),
                effective_port: 8765,
                address_class: BrowserResolvedAddressClassDto::Loopback,
                resolution_fingerprint: format!("hmac-sha256:{}", "b".repeat(64)),
                target_fingerprint: format!("hmac-sha256:{}", "c".repeat(64)),
            },
            risk_kinds: vec![
                BrowserRiskKindDto::InsecureHttp,
                BrowserRiskKindDto::Loopback,
            ],
            trigger: BrowserRiskTriggerDto::ToolArgument,
            dispatch_certainty: BrowserRiskDispatchCertaintyDto::DefinitelyNotDispatched,
            created_at_ms: 1_750_000_000_000,
            expires_at_ms: 1_750_000_900_000,
        };
        let value = serde_json::to_value(&input).unwrap();
        assert_eq!(value["createdAtMs"], 1_750_000_000_000_u64);
        assert_eq!(value["expiresAtMs"], 1_750_000_900_000_u64);
        assert_eq!(value["riskKinds"], json!(["insecure_http", "loopback"]));
        assert_eq!(value["destination"]["addressClass"], "loopback");
        assert_eq!(
            serde_json::from_value::<BrowserRiskAuthorizeInput>(value).unwrap(),
            input
        );
    }

    #[test]
    fn error_wire_uses_the_exact_typescript_casing() {
        let value = serde_json::to_value(ManagedPlaywrightCompletionOutcome::Error {
            code: ManagedPlaywrightBridgeErrorCode::InvalidArguments,
            dispatch_certainty: ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched,
        })
        .unwrap();
        assert_eq!(
            value,
            json!({
                "type": "error",
                "code": "invalid_arguments",
                "dispatchCertainty": "definitely_not_dispatched"
            })
        );
        assert_eq!(
            serde_json::from_value::<ManagedPlaywrightCompletionOutcome>(value).unwrap(),
            ManagedPlaywrightCompletionOutcome::Error {
                code: ManagedPlaywrightBridgeErrorCode::InvalidArguments,
                dispatch_certainty: ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched,
            }
        );

        let capacity = serde_json::to_value(ManagedPlaywrightCompletionOutcome::Error {
            code: ManagedPlaywrightBridgeErrorCode::SurfaceCapacityExceeded,
            dispatch_certainty: ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched,
        })
        .unwrap();
        assert_eq!(capacity["code"], "surface_capacity_exceeded");
    }

    #[test]
    fn sensitive_target_prepare_release_and_grant_wire_are_exact() {
        let input = ManagedPlaywrightPrepareSensitiveToolInput {
            binding_request_id: "7c71eead-3b07-4700-8378-c61d7ea7ac68".to_string(),
            binding_scope: ManagedPlaywrightSensitiveBindingScopeDto::ManagedSurface,
            run_id: "run-1".to_string(),
            capability_id: "browser_automation".to_string(),
            activation_id: "42e7ec2d-03f1-49f3-aa0a-e73ca0f88d91".to_string(),
            manifest_digest: format!("sha256:{}", "a".repeat(64)),
            policy_revision: 7,
            grant_expires_at_ms: 1_750_000_900_000,
            call_id: "call-1".to_string(),
            tool_name: "browser_evaluate".to_string(),
            arguments_digest: format!("sha256:{}", "b".repeat(64)),
            created_at_ms: 1_750_000_000_000,
            expires_at_ms: 1_750_000_600_000,
            file_preparation: None,
        };
        let prepare = serde_json::to_value(ManagedPlaywrightCommand::PrepareSensitiveTool {
            input: Box::new(input.clone()),
        })
        .unwrap();
        assert_eq!(
            prepare,
            json!({
                "type": "prepare_sensitive_tool",
                "input": {
                    "bindingRequestId": "7c71eead-3b07-4700-8378-c61d7ea7ac68",
                    "bindingScope": "managed_surface",
                    "runId": "run-1",
                    "capabilityId": "browser_automation",
                    "activationId": "42e7ec2d-03f1-49f3-aa0a-e73ca0f88d91",
                    "manifestDigest": format!("sha256:{}", "a".repeat(64)),
                    "policyRevision": 7,
                    "grantExpiresAtMs": 1_750_000_900_000_u64,
                    "callId": "call-1",
                    "toolName": "browser_evaluate",
                    "argumentsDigest": format!("sha256:{}", "b".repeat(64)),
                    "createdAtMs": 1_750_000_000_000_u64,
                    "expiresAtMs": 1_750_000_600_000_u64,
                    "filePreparation": null
                }
            })
        );
        assert_eq!(
            serde_json::from_value::<ManagedPlaywrightCommand>(prepare).unwrap(),
            ManagedPlaywrightCommand::PrepareSensitiveTool {
                input: Box::new(input)
            }
        );

        let release = serde_json::to_value(ManagedPlaywrightCommand::ReleaseSensitiveToolBinding {
            binding_id: "398a919a-7b03-4234-b82c-a84498cf18ac".to_string(),
            run_id: "run-1".to_string(),
            activation_id: "42e7ec2d-03f1-49f3-aa0a-e73ca0f88d91".to_string(),
            call_id: "call-1".to_string(),
            reason: ManagedPlaywrightSensitiveBindingReleaseReason::Cancelled,
        })
        .unwrap();
        assert_eq!(release["type"], "release_sensitive_tool_binding");
        assert_eq!(release["bindingId"], "398a919a-7b03-4234-b82c-a84498cf18ac");
        assert_eq!(release["reason"], "cancelled");
        assert!(release.get("surfaceId").is_none());
        assert!(release.get("generation").is_none());

        let grant = ManagedPlaywrightBuiltinToolGrantContext {
            grant_id: "fe9c2750-fddf-4d3e-961f-55f6c0e17395".to_string(),
            approval_id: "ed14d2ba-9dc6-40e0-bfc7-22e71cad7fb8".to_string(),
            arguments_digest: format!("sha256:{}", "b".repeat(64)),
            resource_scope_digest: format!("sha256:{}", "c".repeat(64)),
            target_binding_id: "398a919a-7b03-4234-b82c-a84498cf18ac".to_string(),
            target_binding_digest: format!("sha256:{}", "d".repeat(64)),
            origin: Some("https://mail.example.test".to_string()),
            risk_kinds: vec![BuiltinMcpToolRiskKindDto::PageScriptExecution],
            expires_at_ms: 1_750_000_600_000,
        };
        let grant_value = serde_json::to_value(&grant).unwrap();
        assert_eq!(grant_value["targetBindingId"], grant.target_binding_id);
        assert_eq!(
            grant_value["targetBindingDigest"],
            grant.target_binding_digest
        );
        assert!(grant_value.get("surfaceId").is_none());
        assert_eq!(
            serde_json::from_value::<ManagedPlaywrightBuiltinToolGrantContext>(grant_value)
                .unwrap(),
            grant
        );
    }
}
