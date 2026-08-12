//! Provider-neutral Office automation contracts with a hardened OfficeCLI adapter.
//!
//! The module intentionally keeps process discovery and execution behind an
//! [`OfficeEngine`] boundary. Agent tools can therefore expose stable, typed
//! Office operations without turning a third-party executable into an
//! unrestricted shell capability.

mod discovery;
mod execution;
mod render_runtime;
mod semantic;
mod types;

pub use discovery::{office_cli_component_relative_path, OfficeCliDiscoveryOptions};
pub(crate) use execution::validate_office_request;
pub use execution::{
    DEFAULT_OFFICE_TIMEOUT_MS, MAX_OFFICE_ARGUMENTS, MAX_OFFICE_ARGUMENT_BYTES,
    MAX_OFFICE_DOCUMENT_BYTES, MAX_OFFICE_GRID_COLUMNS, MAX_OFFICE_LIST_VALUES,
    MAX_OFFICE_PROPERTIES, MAX_OFFICE_SCREENSHOT_DIMENSION, MAX_OFFICE_TIMEOUT_MS,
};
pub use render_runtime::{
    office_browser_proxy_mode_requested, office_render_component_relative_path,
    run_office_browser_proxy, OfficeRenderRuntime, OfficeRenderRuntimeDiscoveryOptions,
    OFFICE_RENDER_RUNTIME_BUNDLE_VERSION, OFFICE_RENDER_RUNTIME_PROVIDER_ID,
};
pub use semantic::{
    compile_office_semantic_request, OfficeChartKind, OfficeChartSeries,
    OfficeConditionalFormatKind, OfficeCreateIntent, OfficeDocumentBlockIntent,
    OfficeDocumentBlockKind, OfficeDocumentFormatIntent, OfficeDocumentMoveIntent,
    OfficeDocumentTextIntent, OfficeHeaderFooterIntent, OfficeHorizontalAlignment,
    OfficeImageIntent, OfficeInspectIntent, OfficePresentationChartIntent,
    OfficePresentationFooterIntent, OfficePresentationImageIntent,
    OfficePresentationMoveSlideIntent, OfficePresentationShapeIntent,
    OfficePresentationSlideIndexIntent, OfficePresentationSlideIntent,
    OfficePresentationTableIntent, OfficePresentationTextIntent, OfficeRenderIntent,
    OfficeReplaceTextIntent, OfficeSemanticError, OfficeSemanticErrorCode, OfficeSemanticIntent,
    OfficeSemanticRequest, OfficeSemanticStyle, OfficeSpreadsheetCellIntent,
    OfficeSpreadsheetChartIntent, OfficeSpreadsheetConditionalFormatIntent,
    OfficeSpreadsheetFormulaIntent, OfficeSpreadsheetFreezeIntent, OfficeSpreadsheetImageIntent,
    OfficeSpreadsheetMoveSheetIntent, OfficeSpreadsheetRangeFormatIntent,
    OfficeSpreadsheetSheetIntent, OfficeSpreadsheetTableIntent, OfficeTableIntent,
    OFFICE_SEMANTIC_REQUEST_SCHEMA_VERSION,
};
pub use types::{
    OfficeCellShift, OfficeDocumentKind, OfficeElementPosition, OfficeEngine,
    OfficeEngineAvailability, OfficeEngineCapabilities, OfficeEngineError, OfficeEngineErrorCode,
    OfficeEngineRecovery, OfficeEngineSource, OfficeEngineStatus, OfficeExecutionContext,
    OfficeExecutionRequest, OfficeExecutionResult, OfficeFileState, OfficeFrozenPath,
    OfficeGridLayout, OfficeHelpVerb, OfficeOperation, OfficeOperationAccess,
    OfficeOperationParameters, OfficePageRange, OfficePathIdentity, OfficePathPurpose,
    OfficePathScope, OfficePathSlot, OfficePreparedExecution, OfficePropertyMap,
    OfficePublishedOutput, OfficePublishedOutputKind, OfficePublishedOutputRole,
    OfficeRenderPageSelection, OfficeTextReplacement, OfficeViewMode, OfficeViewRenderMode,
    OfficeViewport, OfficeWriteDisposition, OFFICECLI_PROVIDER_ID,
    OFFICE_ENGINE_STATUS_SCHEMA_VERSION, OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
};

pub use discovery::{resolve_office_engine, OfficeCliEngine, UnavailableOfficeEngine};

#[cfg(all(test, unix))]
mod tests;
