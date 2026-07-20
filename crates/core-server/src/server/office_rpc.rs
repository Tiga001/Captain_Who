use super::*;
use mycopilot_core::office::{
    OfficeDocumentKind, OfficeEngineAvailability, OfficeEngineCapabilities, OfficeEngineSource,
    OfficeEngineStatus, OfficeOperation,
};
use mycopilot_protocol_rs::{
    OfficeDocumentKindDto, OfficeEngineAvailabilityDto, OfficeEngineCapabilitiesDto,
    OfficeEngineSourceDto, OfficeEngineStatusDto, OfficeOperationDto,
};

pub(crate) fn handle_office_status_request(
    agent_service: &AgentService,
    request: JsonRpcRequest,
) -> Value {
    if request.jsonrpc != "2.0" {
        return response_error(Some(request.id), -32600, "Invalid JSON-RPC version");
    }
    if request.params.is_some() {
        return response_error(
            Some(request.id),
            -32602,
            "office.getStatus does not accept parameters",
        );
    }

    response_success(
        request.id,
        office_engine_status_dto(agent_service.get_office_engine_status()),
    )
}

fn office_engine_status_dto(status: OfficeEngineStatus) -> OfficeEngineStatusDto {
    OfficeEngineStatusDto {
        schema_version: status.schema_version,
        provider_id: status.provider_id,
        availability: match status.availability {
            OfficeEngineAvailability::Available => OfficeEngineAvailabilityDto::Available,
            OfficeEngineAvailability::Unavailable => OfficeEngineAvailabilityDto::Unavailable,
        },
        source: status.source.map(|source| match source {
            OfficeEngineSource::Configured => OfficeEngineSourceDto::Configured,
            OfficeEngineSource::PackagedComponent => OfficeEngineSourceDto::PackagedComponent,
            OfficeEngineSource::DevelopmentPath => OfficeEngineSourceDto::DevelopmentPath,
        }),
        version: status.version,
        engine_revision: status.engine_revision,
        capabilities: office_engine_capabilities_dto(status.capabilities),
        error_code: status.error_code,
        message: status.message,
    }
}

fn office_engine_capabilities_dto(
    capabilities: OfficeEngineCapabilities,
) -> OfficeEngineCapabilitiesDto {
    OfficeEngineCapabilitiesDto {
        provider_id: capabilities.provider_id,
        document_kinds: capabilities
            .document_kinds
            .into_iter()
            .map(|kind| match kind {
                OfficeDocumentKind::Document => OfficeDocumentKindDto::Document,
                OfficeDocumentKind::Spreadsheet => OfficeDocumentKindDto::Spreadsheet,
                OfficeDocumentKind::Presentation => OfficeDocumentKindDto::Presentation,
            })
            .collect(),
        operations: capabilities
            .operations
            .into_iter()
            .map(|operation| match operation {
                OfficeOperation::Help => OfficeOperationDto::Help,
                OfficeOperation::Create => OfficeOperationDto::Create,
                OfficeOperation::View => OfficeOperationDto::View,
                OfficeOperation::Get => OfficeOperationDto::Get,
                OfficeOperation::Query => OfficeOperationDto::Query,
                OfficeOperation::Validate => OfficeOperationDto::Validate,
                OfficeOperation::Set => OfficeOperationDto::Set,
                OfficeOperation::Add => OfficeOperationDto::Add,
                OfficeOperation::Remove => OfficeOperationDto::Remove,
                OfficeOperation::Move => OfficeOperationDto::Move,
                OfficeOperation::Swap => OfficeOperationDto::Swap,
            })
            .collect(),
        supports_rendering: capabilities.supports_rendering,
        supports_validation: capabilities.supports_validation,
        supports_structured_output: capabilities.supports_structured_output,
    }
}
