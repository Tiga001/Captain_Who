//! Provider-neutral Office contracts, grouped by operation, execution and outcome.

mod errors;
mod execution;
mod operations;
mod outputs;

pub use errors::{OfficeEngineError, OfficeEngineErrorCode, OfficeEngineRecovery};
pub(crate) use execution::{office_agent_input_placeholder, OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX};
pub use execution::{
    OfficeEngine, OfficeEngineAvailability, OfficeEngineCapabilities, OfficeEngineSource,
    OfficeEngineStatus, OfficeExecutionContext, OfficeExecutionRequest, OfficeFileState,
    OfficeFrozenPath, OfficeManagedScriptBinding, OfficeManagedScriptPurpose, OfficePathIdentity,
    OfficePathPurpose, OfficePathSlot, OfficePreparedExecution, OfficePresentationEditRequest,
    OfficeWriteDisposition, OFFICECLI_PROVIDER_ID, OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
    OFFICE_MANAGED_SCRIPT_BINDING_SCHEMA_VERSION, OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
};
pub use operations::{
    OfficeCellShift, OfficeDocumentKind, OfficeElementPosition, OfficeGridLayout, OfficeHelpVerb,
    OfficeOperation, OfficeOperationAccess, OfficeOperationParameters, OfficePageRange,
    OfficePathScope, OfficePresentationRenderPlan, OfficePropertyMap, OfficeTextReplacement,
    OfficeViewMode, OfficeViewRenderMode, OfficeViewport,
};
pub use outputs::{
    OfficeExecutionResult, OfficeManagedScriptOutputResult, OfficePresentationEditResult,
    OfficePublishedOutput, OfficePublishedOutputKind, OfficePublishedOutputRole,
    OfficeRenderGridGeometry, OfficeRenderLayoutCoverage, OfficeRenderLayoutEvidence,
    OfficeRenderPageSelection,
};

use serde::Deserialize;

fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}
