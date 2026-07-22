use super::*;
use mycopilot_core::image_generation::{
    CredentialSecret, ImageGenerationAdapterId, ImageGenerationConfiguration,
    ImageGenerationConfigurationError, ImageGenerationConfigurationMutationOutcome,
    ImageGenerationConfigurationMutationResult, ImageGenerationConfigurationService,
    ImageGenerationConfigurationUpdate, ImageGenerationCredentialMutation,
    ImageGenerationCredentialStatus, ImageGenerationDefaults, ImageGenerationReadiness,
    ImageGenerationSizePreset,
};
use mycopilot_protocol_rs::{
    ImageGenerationAdapterIdDto, ImageGenerationCapabilitiesDto, ImageGenerationConfigurationDto,
    ImageGenerationConfigurationErrorCodeDto, ImageGenerationConfigurationErrorData,
    ImageGenerationConfigurationErrorTypeDto, ImageGenerationConfigurationMutationOutcomeDto,
    ImageGenerationConfigurationOperationDto, ImageGenerationConfigurationRecoveryDto,
    ImageGenerationCredentialMutationDto, ImageGenerationCredentialStatusDto,
    ImageGenerationDefaultsDto, ImageGenerationGetConfigurationResponse,
    ImageGenerationReadinessDto, ImageGenerationSetEnabledRequest, ImageGenerationSizePresetDto,
    ImageGenerationStatusDto, ImageGenerationUpdateConfigurationRequest,
    ImageGenerationUpdateConfigurationResponse, IMAGE_GENERATION_CONFIGURATION_ERROR_CODE,
    IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
};

pub(crate) fn image_generation_configuration_operation(
    method: &str,
) -> Option<ImageGenerationConfigurationOperationDto> {
    match method {
        IMAGE_GENERATION_GET_CONFIGURATION_METHOD => {
            Some(ImageGenerationConfigurationOperationDto::GetConfiguration)
        }
        IMAGE_GENERATION_UPDATE_CONFIGURATION_METHOD => {
            Some(ImageGenerationConfigurationOperationDto::UpdateConfiguration)
        }
        IMAGE_GENERATION_SET_ENABLED_METHOD => {
            Some(ImageGenerationConfigurationOperationDto::SetEnabled)
        }
        IMAGE_GENERATION_GET_STATUS_METHOD => {
            Some(ImageGenerationConfigurationOperationDto::GetStatus)
        }
        _ => None,
    }
}

pub(crate) fn handle_image_generation_configuration_request(
    service: &ImageGenerationConfigurationService,
    request: JsonRpcRequest,
) -> Value {
    if request.jsonrpc != "2.0" {
        return response_error(Some(request.id), -32600, "Invalid JSON-RPC version");
    }

    let operation = match request.method.as_str() {
        IMAGE_GENERATION_GET_CONFIGURATION_METHOD => {
            let id = request.id;
            if request.params.is_some() {
                return image_generation_configuration_error_response(
                    id,
                    ImageGenerationConfigurationOperationDto::GetConfiguration,
                    ImageGenerationConfigurationError::InvalidConfiguration,
                );
            }
            return match service.get_configuration() {
                Ok(configuration) => response_success(
                    id,
                    ImageGenerationGetConfigurationResponse {
                        schema_version: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
                        configuration: configuration_dto(configuration),
                    },
                ),
                Err(error) => image_generation_configuration_error_response(
                    id,
                    ImageGenerationConfigurationOperationDto::GetConfiguration,
                    error,
                ),
            };
        }
        IMAGE_GENERATION_UPDATE_CONFIGURATION_METHOD => {
            ImageGenerationConfigurationOperationDto::UpdateConfiguration
        }
        IMAGE_GENERATION_SET_ENABLED_METHOD => ImageGenerationConfigurationOperationDto::SetEnabled,
        IMAGE_GENERATION_GET_STATUS_METHOD => {
            let id = request.id;
            if request.params.is_some() {
                return image_generation_configuration_error_response(
                    id,
                    ImageGenerationConfigurationOperationDto::GetStatus,
                    ImageGenerationConfigurationError::InvalidConfiguration,
                );
            }
            return match service.get_configuration() {
                Ok(configuration) => response_success(id, status_dto(configuration)),
                Err(error) => image_generation_configuration_error_response(
                    id,
                    ImageGenerationConfigurationOperationDto::GetStatus,
                    error,
                ),
            };
        }
        _ => return response_error(Some(request.id), -32601, "Method not found"),
    };

    let id = request.id;
    let result = match operation {
        ImageGenerationConfigurationOperationDto::UpdateConfiguration => {
            let input =
                match parse_params::<ImageGenerationUpdateConfigurationRequest>(request.params) {
                    Ok(input) => input,
                    Err(_) => {
                        return image_generation_configuration_error_response(
                            id,
                            operation,
                            ImageGenerationConfigurationError::InvalidConfiguration,
                        )
                    }
                };
            if input.schema_version != IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION {
                return image_generation_configuration_error_response(
                    id,
                    operation,
                    ImageGenerationConfigurationError::InvalidConfiguration,
                );
            }
            let update = match configuration_update(input) {
                Ok(update) => update,
                Err(error) => {
                    return image_generation_configuration_error_response(id, operation, error)
                }
            };
            service.update_configuration(update)
        }
        ImageGenerationConfigurationOperationDto::SetEnabled => {
            let input = match parse_params::<ImageGenerationSetEnabledRequest>(request.params) {
                Ok(input) => input,
                Err(_) => {
                    return image_generation_configuration_error_response(
                        id,
                        operation,
                        ImageGenerationConfigurationError::InvalidConfiguration,
                    )
                }
            };
            if input.schema_version != IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION {
                return image_generation_configuration_error_response(
                    id,
                    operation,
                    ImageGenerationConfigurationError::InvalidConfiguration,
                );
            }
            service.set_enabled(&input.expected_revision, input.enabled)
        }
        ImageGenerationConfigurationOperationDto::GetConfiguration
        | ImageGenerationConfigurationOperationDto::GetStatus => {
            unreachable!("read operations return before mutation dispatch")
        }
    };

    match result {
        Ok(result) => response_success(id, mutation_response(result)),
        Err(error) => image_generation_configuration_error_response(id, operation, error),
    }
}

fn configuration_update(
    input: ImageGenerationUpdateConfigurationRequest,
) -> Result<ImageGenerationConfigurationUpdate, ImageGenerationConfigurationError> {
    let credential_mutation = match input.credential_mutation {
        ImageGenerationCredentialMutationDto::Keep => ImageGenerationCredentialMutation::Keep,
        ImageGenerationCredentialMutationDto::Replace { value } => {
            let secret = CredentialSecret::new(value)
                .map_err(|_| ImageGenerationConfigurationError::InvalidCredential)?;
            ImageGenerationCredentialMutation::Replace(secret)
        }
        ImageGenerationCredentialMutationDto::Clear => ImageGenerationCredentialMutation::Clear,
    };
    Ok(ImageGenerationConfigurationUpdate {
        expected_revision: input.expected_revision,
        adapter_id: match input.adapter_id {
            ImageGenerationAdapterIdDto::SmartMlSeedream => {
                ImageGenerationAdapterId::SmartMlSeedream
            }
        },
        endpoint_url: input.endpoint_url,
        model_id: input.model_id,
        text_to_image: input.capabilities.text_to_image,
        image_to_image: input.capabilities.image_to_image,
        defaults: ImageGenerationDefaults {
            size_preset: match input.defaults.size_preset {
                ImageGenerationSizePresetDto::TwoK => ImageGenerationSizePreset::TwoK,
            },
            watermark: input.defaults.watermark,
        },
        credential_mutation,
    })
}

fn mutation_response(
    result: ImageGenerationConfigurationMutationResult,
) -> ImageGenerationUpdateConfigurationResponse {
    ImageGenerationUpdateConfigurationResponse {
        schema_version: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
        outcome: match result.outcome {
            ImageGenerationConfigurationMutationOutcome::Updated => {
                ImageGenerationConfigurationMutationOutcomeDto::Updated
            }
            ImageGenerationConfigurationMutationOutcome::AlreadyCurrent => {
                ImageGenerationConfigurationMutationOutcomeDto::AlreadyCurrent
            }
        },
        configuration: configuration_dto(result.configuration),
    }
}

fn status_dto(configuration: ImageGenerationConfiguration) -> ImageGenerationStatusDto {
    let ImageGenerationConfiguration {
        adapter_id,
        capabilities,
        defaults,
        credential_status,
        enabled,
        readiness,
        revision,
        ..
    } = configuration;
    ImageGenerationStatusDto {
        schema_version: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
        adapter_id: adapter_id_dto(adapter_id),
        configuration_revision: revision,
        enabled,
        readiness: readiness_dto(readiness),
        credential_status: credential_status_dto(credential_status),
        capabilities: ImageGenerationCapabilitiesDto {
            text_to_image: capabilities.text_to_image,
            image_to_image: capabilities.image_to_image,
        },
        defaults: defaults_dto(defaults),
    }
}

fn configuration_dto(
    configuration: ImageGenerationConfiguration,
) -> ImageGenerationConfigurationDto {
    ImageGenerationConfigurationDto {
        adapter_id: adapter_id_dto(configuration.adapter_id),
        endpoint_url: configuration.endpoint_url,
        model_id: configuration.model_id,
        capabilities: ImageGenerationCapabilitiesDto {
            text_to_image: configuration.capabilities.text_to_image,
            image_to_image: configuration.capabilities.image_to_image,
        },
        defaults: defaults_dto(configuration.defaults),
        credential_status: credential_status_dto(configuration.credential_status),
        enabled: configuration.enabled,
        readiness: readiness_dto(configuration.readiness),
        revision: configuration.revision,
    }
}

fn adapter_id_dto(adapter_id: ImageGenerationAdapterId) -> ImageGenerationAdapterIdDto {
    match adapter_id {
        ImageGenerationAdapterId::SmartMlSeedream => ImageGenerationAdapterIdDto::SmartMlSeedream,
    }
}

fn defaults_dto(defaults: ImageGenerationDefaults) -> ImageGenerationDefaultsDto {
    ImageGenerationDefaultsDto {
        size_preset: match defaults.size_preset {
            ImageGenerationSizePreset::TwoK => ImageGenerationSizePresetDto::TwoK,
        },
        watermark: defaults.watermark,
    }
}

fn credential_status_dto(
    status: ImageGenerationCredentialStatus,
) -> ImageGenerationCredentialStatusDto {
    match status {
        ImageGenerationCredentialStatus::Missing => ImageGenerationCredentialStatusDto::Missing,
        ImageGenerationCredentialStatus::Configured => {
            ImageGenerationCredentialStatusDto::Configured
        }
        ImageGenerationCredentialStatus::Unavailable => {
            ImageGenerationCredentialStatusDto::Unavailable
        }
    }
}

fn readiness_dto(readiness: ImageGenerationReadiness) -> ImageGenerationReadinessDto {
    match readiness {
        ImageGenerationReadiness::Disabled => ImageGenerationReadinessDto::Disabled,
        ImageGenerationReadiness::MissingEndpoint => ImageGenerationReadinessDto::MissingEndpoint,
        ImageGenerationReadiness::MissingModel => ImageGenerationReadinessDto::MissingModel,
        ImageGenerationReadiness::MissingCredential => {
            ImageGenerationReadinessDto::MissingCredential
        }
        ImageGenerationReadiness::CredentialUnavailable => {
            ImageGenerationReadinessDto::CredentialUnavailable
        }
        ImageGenerationReadiness::ReadyUnverified => ImageGenerationReadinessDto::ReadyUnverified,
    }
}

pub(crate) fn image_generation_configuration_error_response(
    id: JsonRpcId,
    operation: ImageGenerationConfigurationOperationDto,
    error: ImageGenerationConfigurationError,
) -> Value {
    let (code, recovery) = match &error {
        ImageGenerationConfigurationError::InvalidRevision
        | ImageGenerationConfigurationError::TextToImageRequired
        | ImageGenerationConfigurationError::ConfigurationIncomplete(_)
        | ImageGenerationConfigurationError::InvalidConfiguration => (
            ImageGenerationConfigurationErrorCodeDto::InvalidRequest,
            ImageGenerationConfigurationRecoveryDto::FixConfiguration,
        ),
        ImageGenerationConfigurationError::RevisionConflict { .. } => (
            ImageGenerationConfigurationErrorCodeDto::RevisionConflict,
            ImageGenerationConfigurationRecoveryDto::RefreshConfiguration,
        ),
        ImageGenerationConfigurationError::UnsupportedAdapter => (
            ImageGenerationConfigurationErrorCodeDto::UnsupportedAdapter,
            ImageGenerationConfigurationRecoveryDto::FixConfiguration,
        ),
        ImageGenerationConfigurationError::MissingEndpoint => (
            ImageGenerationConfigurationErrorCodeDto::MissingEndpoint,
            ImageGenerationConfigurationRecoveryDto::FixConfiguration,
        ),
        ImageGenerationConfigurationError::InvalidEndpoint => (
            ImageGenerationConfigurationErrorCodeDto::InvalidEndpoint,
            ImageGenerationConfigurationRecoveryDto::FixConfiguration,
        ),
        ImageGenerationConfigurationError::InsecureEndpoint => (
            ImageGenerationConfigurationErrorCodeDto::InsecureEndpoint,
            ImageGenerationConfigurationRecoveryDto::FixConfiguration,
        ),
        ImageGenerationConfigurationError::MissingModel => (
            ImageGenerationConfigurationErrorCodeDto::MissingModel,
            ImageGenerationConfigurationRecoveryDto::FixConfiguration,
        ),
        ImageGenerationConfigurationError::InvalidModelId => (
            ImageGenerationConfigurationErrorCodeDto::InvalidModelId,
            ImageGenerationConfigurationRecoveryDto::FixConfiguration,
        ),
        ImageGenerationConfigurationError::MissingCredential => (
            ImageGenerationConfigurationErrorCodeDto::MissingCredential,
            ImageGenerationConfigurationRecoveryDto::ReenterCredential,
        ),
        ImageGenerationConfigurationError::InvalidCredential => (
            ImageGenerationConfigurationErrorCodeDto::InvalidCredential,
            ImageGenerationConfigurationRecoveryDto::ReenterCredential,
        ),
        ImageGenerationConfigurationError::CredentialReplacementRequired => (
            ImageGenerationConfigurationErrorCodeDto::CredentialReplacementRequired,
            ImageGenerationConfigurationRecoveryDto::ReenterCredential,
        ),
        ImageGenerationConfigurationError::StorageUnavailable => (
            ImageGenerationConfigurationErrorCodeDto::StorageUnavailable,
            ImageGenerationConfigurationRecoveryDto::Retry,
        ),
        ImageGenerationConfigurationError::CredentialStoreUnavailable => (
            ImageGenerationConfigurationErrorCodeDto::CredentialStoreUnavailable,
            ImageGenerationConfigurationRecoveryDto::Retry,
        ),
        ImageGenerationConfigurationError::CommitIndeterminate => (
            ImageGenerationConfigurationErrorCodeDto::CommitIndeterminate,
            ImageGenerationConfigurationRecoveryDto::RefreshConfiguration,
        ),
        ImageGenerationConfigurationError::Unavailable => (
            ImageGenerationConfigurationErrorCodeDto::Unavailable,
            ImageGenerationConfigurationRecoveryDto::Retry,
        ),
    };
    let data = ImageGenerationConfigurationErrorData {
        error_type: ImageGenerationConfigurationErrorTypeDto::ImageGenerationConfiguration,
        operation,
        code,
        recovery,
        message: error.to_string(),
        current_revision: match &error {
            ImageGenerationConfigurationError::RevisionConflict { current_revision } => {
                Some(current_revision.clone())
            }
            _ => None,
        },
        retry_after_ms: None,
        configuration_may_have_changed: matches!(
            error,
            ImageGenerationConfigurationError::CommitIndeterminate
        )
        .then_some(true),
    };
    serde_json::to_value(error_with_data(
        Some(id),
        IMAGE_GENERATION_CONFIGURATION_ERROR_CODE,
        data.message.clone(),
        serde_json::to_value(data).expect("image-generation error data must serialize"),
    ))
    .expect("JSON-RPC error response must serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::image_generation::InMemoryCredentialStore;

    struct Fixture {
        _temp: tempfile::TempDir,
        service: ImageGenerationConfigurationService,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let storage = Arc::new(
                StorageService::open(&temp.path().join("image-generation.sqlite")).unwrap(),
            );
            Self {
                _temp: temp,
                service: ImageGenerationConfigurationService::new(
                    storage,
                    Arc::new(InMemoryCredentialStore::default()),
                ),
            }
        }

        fn request(&self, id: i64, method: &str, params: Option<Value>) -> Value {
            handle_image_generation_configuration_request(
                &self.service,
                JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    id: JsonRpcId::Number(id),
                    method: method.to_string(),
                    params,
                },
            )
        }
    }

    fn valid_update(expected_revision: &str, credential: &str) -> Value {
        json!({
            "schemaVersion": IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
            "expectedRevision": expected_revision,
            "adapterId": "smartmlSeedream",
            "endpointUrl": "https://zju.smartml.cn/userapi/v1/images/generations",
            "modelId": "doubao-seedream-4-0-250828",
            "capabilities": {
                "textToImage": true,
                "imageToImage": true
            },
            "defaults": {
                "sizePreset": "2K",
                "watermark": true
            },
            "credentialMutation": {
                "type": "replace",
                "value": credential
            }
        })
    }

    #[test]
    fn configuration_lifecycle_never_returns_the_credential() {
        let fixture = Fixture::new();
        let initial = fixture.request(1, IMAGE_GENERATION_GET_CONFIGURATION_METHOD, None);
        assert_eq!(
            initial["result"]["configuration"]["revision"],
            "image-generation:v1:0"
        );
        assert_eq!(initial["result"]["configuration"]["readiness"], "disabled");

        let secret = "provider-secret-that-must-never-cross-the-response-boundary";
        let updated = fixture.request(
            2,
            IMAGE_GENERATION_UPDATE_CONFIGURATION_METHOD,
            Some(valid_update("image-generation:v1:0", secret)),
        );
        assert_eq!(updated["result"]["outcome"], "updated");
        assert_eq!(
            updated["result"]["configuration"]["credentialStatus"],
            "configured"
        );
        assert_eq!(updated["result"]["configuration"]["enabled"], false);
        assert!(!updated.to_string().contains(secret));

        let enabled = fixture.request(
            3,
            IMAGE_GENERATION_SET_ENABLED_METHOD,
            Some(json!({
                "schemaVersion": IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
                "expectedRevision": updated["result"]["configuration"]["revision"],
                "enabled": true
            })),
        );
        assert_eq!(
            enabled["result"]["configuration"]["readiness"],
            "readyUnverified"
        );

        let status = fixture.request(4, IMAGE_GENERATION_GET_STATUS_METHOD, None);
        assert_eq!(status["result"]["enabled"], true);
        assert_eq!(status["result"]["capabilities"]["textToImage"], true);
        assert_eq!(status["result"]["capabilities"]["imageToImage"], true);
        assert!(status["result"].get("endpointUrl").is_none());
        assert!(status["result"].get("modelId").is_none());
    }

    #[test]
    fn stale_update_returns_strict_refreshable_conflict() {
        let fixture = Fixture::new();
        let first = fixture.request(
            1,
            IMAGE_GENERATION_UPDATE_CONFIGURATION_METHOD,
            Some(valid_update("image-generation:v1:0", "first-secret")),
        );
        assert_eq!(first["result"]["outcome"], "updated");

        let stale = fixture.request(
            2,
            IMAGE_GENERATION_UPDATE_CONFIGURATION_METHOD,
            Some(valid_update("image-generation:v1:0", "stale-secret")),
        );
        assert_eq!(
            stale["error"]["code"],
            IMAGE_GENERATION_CONFIGURATION_ERROR_CODE
        );
        assert_eq!(
            stale["error"]["data"]["type"],
            "imageGenerationConfiguration"
        );
        assert_eq!(stale["error"]["data"]["code"], "revisionConflict");
        assert_eq!(stale["error"]["data"]["recovery"], "refreshConfiguration");
        assert_eq!(
            stale["error"]["data"]["currentRevision"],
            "image-generation:v1:1"
        );
        assert!(!stale.to_string().contains("stale-secret"));
    }

    #[test]
    fn parameter_shape_errors_use_the_same_redacted_structured_contract() {
        let fixture = Fixture::new();
        for method in [
            IMAGE_GENERATION_GET_CONFIGURATION_METHOD,
            IMAGE_GENERATION_GET_STATUS_METHOD,
        ] {
            let response = fixture.request(1, method, Some(json!({})));
            assert_eq!(
                response["error"]["code"],
                IMAGE_GENERATION_CONFIGURATION_ERROR_CODE
            );
            assert_eq!(response["error"]["data"]["code"], "invalidRequest");
            assert_eq!(response["error"]["data"]["recovery"], "fixConfiguration");
        }

        let secret = "secret-that-must-not-appear-in-a-shape-error";
        let malformed = fixture.request(
            2,
            IMAGE_GENERATION_UPDATE_CONFIGURATION_METHOD,
            Some(json!({
                "unexpectedCredential": secret
            })),
        );
        assert_eq!(malformed["error"]["data"]["code"], "invalidRequest");
        assert!(!malformed.to_string().contains(secret));
    }
}
