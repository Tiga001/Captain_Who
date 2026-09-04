use super::*;

#[derive(Debug, Clone)]
pub struct AgentError {
    message: String,
    cancelled: bool,
    usage: Option<Box<AgentUsage>>,
    code: Option<String>,
    details: Option<Box<Value>>,
    conversation_turn_trace: Option<Box<ConversationTurnTrace>>,
    model_request_observation: Option<Box<crate::ModelRequestObservation>>,
    // Set only at the runtime's final model-request boundary. This proves the failed work was
    // provisional model sampling, so the Host may safely keep the committed trace prefix.
    model_request_interruption: Option<AgentModelRequestInterruptionReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentModelRequestInterruptionReason {
    ServiceConnectionFailed,
    ServiceUnavailable,
    AuthenticationFailed,
    QuotaExhausted,
    ContextLimitExceeded,
    RequestRejected,
    ResponseInvalid,
    RequestFailed,
}

impl AgentModelRequestInterruptionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ServiceConnectionFailed => "service_connection_failed",
            Self::ServiceUnavailable => "service_unavailable",
            Self::AuthenticationFailed => "authentication_failed",
            Self::QuotaExhausted => "quota_exhausted",
            Self::ContextLimitExceeded => "context_limit_exceeded",
            Self::RequestRejected => "request_rejected",
            Self::ResponseInvalid => "response_invalid",
            Self::RequestFailed => "request_failed",
        }
    }
}

pub type AgentResult<T> = Result<T, AgentError>;

impl AgentError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cancelled: false,
            usage: None,
            code: None,
            details: None,
            conversation_turn_trace: None,
            model_request_observation: None,
            model_request_interruption: None,
        }
    }

    pub fn structured(code: impl Into<String>, message: impl Into<String>, details: Value) -> Self {
        Self {
            message: message.into(),
            cancelled: false,
            usage: None,
            code: Some(code.into()),
            details: Some(Box::new(details)),
            conversation_turn_trace: None,
            model_request_observation: None,
            model_request_interruption: None,
        }
    }

    pub fn cancelled() -> Self {
        Self {
            message: "agent run 已取消。".to_string(),
            cancelled: true,
            usage: None,
            code: None,
            details: None,
            conversation_turn_trace: None,
            model_request_observation: None,
            model_request_interruption: None,
        }
    }

    /// A cancellation with a bounded typed receipt. This is used when the caller can prove a
    /// dispatch boundary (for example, an approved MCP call cancelled before Host dispatch)
    /// without degrading the normal cancellation control flow into an ordinary Tool failure.
    pub fn cancelled_structured(
        code: impl Into<String>,
        message: impl Into<String>,
        details: Value,
    ) -> Self {
        Self {
            message: message.into(),
            cancelled: true,
            usage: None,
            code: Some(code.into()),
            details: Some(Box::new(details)),
            conversation_turn_trace: None,
            model_request_observation: None,
            model_request_interruption: None,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn usage(&self) -> Option<&AgentUsage> {
        self.usage.as_deref()
    }

    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    pub fn details(&self) -> Option<&Value> {
        self.details.as_deref()
    }

    pub fn conversation_turn_trace(&self) -> Option<&ConversationTurnTrace> {
        self.conversation_turn_trace.as_deref()
    }

    pub fn model_request_observation(&self) -> Option<&crate::ModelRequestObservation> {
        self.model_request_observation.as_deref()
    }

    pub fn model_request_interruption(&self) -> Option<AgentModelRequestInterruptionReason> {
        self.model_request_interruption
    }

    pub fn with_usage(mut self, usage: Option<AgentUsage>) -> Self {
        self.usage = usage.map(Box::new);
        self
    }

    pub fn with_conversation_turn_trace(mut self, trace: ConversationTurnTrace) -> Self {
        self.conversation_turn_trace = Some(Box::new(trace));
        self
    }

    pub fn with_model_request_observation(
        mut self,
        observation: crate::ModelRequestObservation,
    ) -> Self {
        self.model_request_observation = Some(Box::new(observation));
        self
    }

    pub fn with_model_request_interruption(mut self) -> Self {
        self.model_request_interruption = Some(classify_model_request_interruption(&self));
        self
    }
}

fn classify_model_request_interruption(error: &AgentError) -> AgentModelRequestInterruptionReason {
    if error.code() == Some("agent.llm_provider_failure") {
        return match error
            .details()
            .and_then(|details| details.get("category"))
            .and_then(Value::as_str)
        {
            Some("network") => AgentModelRequestInterruptionReason::ServiceConnectionFailed,
            Some("rate_limited" | "overloaded") => {
                AgentModelRequestInterruptionReason::ServiceUnavailable
            }
            Some("authentication") => AgentModelRequestInterruptionReason::AuthenticationFailed,
            Some("quota_exhausted") => AgentModelRequestInterruptionReason::QuotaExhausted,
            Some("context_too_large") => AgentModelRequestInterruptionReason::ContextLimitExceeded,
            Some("invalid_request") => AgentModelRequestInterruptionReason::RequestRejected,
            _ => AgentModelRequestInterruptionReason::RequestFailed,
        };
    }

    if error.code().is_some_and(|code| {
        code.contains("response") || code.contains("stream") || code.contains("decode")
    }) {
        AgentModelRequestInterruptionReason::ResponseInvalid
    } else {
        AgentModelRequestInterruptionReason::RequestFailed
    }
}

impl Display for AgentError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for AgentError {}

impl From<String> for AgentError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for AgentError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}
