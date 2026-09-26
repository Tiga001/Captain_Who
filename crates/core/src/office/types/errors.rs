use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OfficeEngineErrorCode {
    Unavailable,
    InvalidConfiguration,
    RenderBackendUnavailable,
    RenderBackendInvalid,
    RenderBackendTimeout,
    RenderBackendFailed,
    InvalidRequest,
    UnsafeOperation,
    UnsupportedOperation,
    WorkspaceViolation,
    PreconditionFailed,
    InvalidOutput,
    CommitIndeterminate,
    Io,
    ProcessFailure,
}

impl OfficeEngineErrorCode {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::Unavailable => "office.engine_unavailable",
            Self::InvalidConfiguration => "office.invalid_configuration",
            Self::RenderBackendUnavailable => "office.render_backend_unavailable",
            Self::RenderBackendInvalid => "office.render_backend_invalid",
            Self::RenderBackendTimeout => "office.render_backend_timeout",
            Self::RenderBackendFailed => "office.render_backend_failed",
            Self::InvalidRequest => "office.invalid_request",
            Self::UnsafeOperation => "office.unsafe_operation",
            Self::UnsupportedOperation => "office.unsupported_operation",
            Self::WorkspaceViolation => "office.workspace_violation",
            Self::PreconditionFailed => "office.precondition_failed",
            Self::InvalidOutput => "office.invalid_output",
            Self::CommitIndeterminate => "office.commit_indeterminate",
            Self::Io => "office.io",
            Self::ProcessFailure => "office.process_failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OfficeEngineRecovery {
    InstallComponent,
    ChangeConfiguration,
    ChangeRequest,
    Retry,
    InspectState,
}

impl OfficeEngineRecovery {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::InstallComponent => "installComponent",
            Self::ChangeConfiguration => "changeConfiguration",
            Self::ChangeRequest => "changeRequest",
            Self::Retry => "retry",
            Self::InspectState => "inspectState",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficeEngineError {
    code: OfficeEngineErrorCode,
    recovery: OfficeEngineRecovery,
    message: String,
}

impl OfficeEngineError {
    /// Creates a stable provider-facing error for an [`OfficeEngine`]
    /// implementation. Callers should choose the narrowest stable code and a
    /// recovery action that can be safely shown to the user.
    pub fn new(
        code: OfficeEngineErrorCode,
        recovery: OfficeEngineRecovery,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            recovery,
            message: message.into(),
        }
    }

    pub fn code(&self) -> OfficeEngineErrorCode {
        self.code
    }

    pub fn recovery(&self) -> OfficeEngineRecovery {
        self.recovery
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for OfficeEngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl Error for OfficeEngineError {}
