//! Provider-neutral Office automation contracts with a hardened OfficeCLI adapter.
//!
//! The module intentionally keeps process discovery and execution behind an
//! [`OfficeEngine`] boundary. Agent tools can therefore expose stable, typed
//! Office operations without turning a third-party executable into an
//! unrestricted shell capability.

mod discovery;
mod execution;
mod types;

pub use discovery::{office_cli_component_relative_path, OfficeCliDiscoveryOptions};
pub(crate) use execution::validate_office_request;
pub use execution::{
    DEFAULT_OFFICE_TIMEOUT_MS, MAX_OFFICE_ARGUMENTS, MAX_OFFICE_ARGUMENT_BYTES,
    MAX_OFFICE_DOCUMENT_BYTES, MAX_OFFICE_GRID_COLUMNS, MAX_OFFICE_LIST_VALUES,
    MAX_OFFICE_PROPERTIES, MAX_OFFICE_SCREENSHOT_DIMENSION, MAX_OFFICE_TIMEOUT_MS,
};
pub use types::{
    OfficeCellShift, OfficeDocumentKind, OfficeElementPosition, OfficeEngine,
    OfficeEngineAvailability, OfficeEngineCapabilities, OfficeEngineError, OfficeEngineErrorCode,
    OfficeEngineRecovery, OfficeEngineSource, OfficeEngineStatus, OfficeExecutionContext,
    OfficeExecutionRequest, OfficeExecutionResult, OfficeFilePrecondition, OfficeFileState,
    OfficeFrozenPath, OfficeGridLayout, OfficeHelpVerb, OfficeOperation, OfficeOperationAccess,
    OfficeOperationParameters, OfficePageRange, OfficePathIdentity, OfficePathPurpose,
    OfficePathScope, OfficePathSlot, OfficePreparedExecution, OfficePropertyMap,
    OfficeRequestParameters, OfficeTextReplacement, OfficeViewMode, OfficeViewRenderMode,
    OfficeViewport, OfficeWriteDisposition, OFFICECLI_PROVIDER_ID,
    OFFICE_ENGINE_STATUS_SCHEMA_VERSION, OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
};

pub use discovery::{resolve_office_engine, OfficeCliEngine, UnavailableOfficeEngine};

#[cfg(all(test, unix))]
mod tests;
