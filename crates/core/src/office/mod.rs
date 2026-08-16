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
mod word_pdf_render_runtime;

pub use discovery::{office_cli_component_relative_path, OfficeCliDiscoveryOptions};
pub(crate) use execution::{
    prepare_managed_script_binding, prepare_managed_script_staging, validate_office_request,
};
pub use execution::{
    OfficeManagedScriptStaging, DEFAULT_OFFICE_TIMEOUT_MS, MAX_OFFICE_ARGUMENTS,
    MAX_OFFICE_ARGUMENT_BYTES, MAX_OFFICE_DOCUMENT_BYTES, MAX_OFFICE_GRID_COLUMNS,
    MAX_OFFICE_LIST_VALUES, MAX_OFFICE_PAGE_NUMBER, MAX_OFFICE_PROPERTIES,
    MAX_OFFICE_SCREENSHOT_DIMENSION, MAX_OFFICE_TIMEOUT_MS, MAX_OFFICE_TOTAL_PAGES,
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
    OfficeRenderOutputFormat, OfficeReplaceTextIntent, OfficeSemanticError,
    OfficeSemanticErrorCode, OfficeSemanticIntent, OfficeSemanticRequest, OfficeSemanticStyle,
    OfficeSpreadsheetCellIntent, OfficeSpreadsheetChartIntent,
    OfficeSpreadsheetConditionalFormatIntent, OfficeSpreadsheetFormulaIntent,
    OfficeSpreadsheetFreezeIntent, OfficeSpreadsheetImageIntent, OfficeSpreadsheetMoveSheetIntent,
    OfficeSpreadsheetRangeFormatIntent, OfficeSpreadsheetSheetIntent, OfficeSpreadsheetTableIntent,
    OfficeTableIntent, OFFICE_SEMANTIC_REQUEST_SCHEMA_VERSION,
};
pub use types::{
    OfficeCellShift, OfficeDocumentKind, OfficeElementPosition, OfficeEngine,
    OfficeEngineAvailability, OfficeEngineCapabilities, OfficeEngineError, OfficeEngineErrorCode,
    OfficeEngineRecovery, OfficeEngineSource, OfficeEngineStatus, OfficeExecutionContext,
    OfficeExecutionRequest, OfficeExecutionResult, OfficeFileState, OfficeFrozenPath,
    OfficeGridLayout, OfficeHelpVerb, OfficeManagedScriptBinding, OfficeManagedScriptOutputResult,
    OfficeManagedScriptPurpose, OfficeOperation, OfficeOperationAccess, OfficeOperationParameters,
    OfficePageRange, OfficePathIdentity, OfficePathPurpose, OfficePathScope, OfficePathSlot,
    OfficePreparedExecution, OfficePresentationEditRequest, OfficePresentationEditResult,
    OfficePresentationRenderPlan, OfficePropertyMap, OfficePublishedOutput,
    OfficePublishedOutputKind, OfficePublishedOutputRole, OfficeRenderGridGeometry,
    OfficeRenderLayoutCoverage, OfficeRenderLayoutEvidence, OfficeRenderPageSelection,
    OfficeTextReplacement, OfficeViewMode, OfficeViewRenderMode, OfficeViewport,
    OfficeWriteDisposition, OFFICECLI_PROVIDER_ID, OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
    OFFICE_MANAGED_SCRIPT_BINDING_SCHEMA_VERSION, OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
};
pub use word_pdf_render_runtime::{
    word_pdf_render_component_relative_path, WordPdfRenderRuntime,
    WordPdfRenderRuntimeDiscoveryOptions, WORD_PDF_RENDER_RUNTIME_BUNDLE_VERSION,
    WORD_PDF_RENDER_RUNTIME_LIBREOFFICE_VERSION, WORD_PDF_RENDER_RUNTIME_PROVIDER_ID,
};

pub use discovery::{resolve_office_engine, OfficeCliEngine, UnavailableOfficeEngine};
pub(crate) use types::OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX;

#[cfg(all(test, unix))]
mod tests;
