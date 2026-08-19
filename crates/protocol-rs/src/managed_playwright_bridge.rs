use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION: u32 = 1;
pub const MANAGED_PLAYWRIGHT_COMMAND_NOTIFICATION_METHOD: &str = "mcp.builtinPlaywright.command";
pub const MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD: &str = "mcp.builtinPlaywright.cancel";
pub const MANAGED_PLAYWRIGHT_COMPLETE_METHOD: &str = "mcp.builtinPlaywright.complete";
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
    fn call_tool_wire_uses_the_exact_typescript_casing() {
        let authorization_context = ManagedPlaywrightAuthorizationContext {
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
}
