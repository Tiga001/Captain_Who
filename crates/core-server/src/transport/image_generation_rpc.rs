use super::*;
use base64::Engine;
use mycopilot_core::image_generation::{
    CredentialSecret, ImageArtifactErrorCode, ImageArtifactFormat, ImageGenerationAdapterId,
    ImageGenerationArtifactCandidate, ImageGenerationConfiguration,
    ImageGenerationConfigurationError, ImageGenerationConfigurationMutationOutcome,
    ImageGenerationConfigurationMutationResult, ImageGenerationConfigurationService,
    ImageGenerationConfigurationUpdate, ImageGenerationCredentialMutation,
    ImageGenerationCredentialStatus, ImageGenerationDefaults, ImageGenerationReadiness,
    ImageGenerationSizePreset, ManagedImageGenerationArtifactStore,
    DEFAULT_IMAGE_ARTIFACT_MAX_BYTES, MAX_IMAGE_ARTIFACT_DIMENSION, MAX_IMAGE_ARTIFACT_PIXELS,
};
#[cfg(test)]
use mycopilot_protocol_rs::IMAGE_GENERATION_CONFIGURATION_ERROR_CODE;
use mycopilot_protocol_rs::{
    ImageGenerationAdapterIdDto, ImageGenerationArtifactDto, ImageGenerationArtifactErrorCodeDto,
    ImageGenerationArtifactErrorData, ImageGenerationArtifactErrorTypeDto,
    ImageGenerationArtifactFormatDto, ImageGenerationArtifactKindDto,
    ImageGenerationArtifactOperationDto, ImageGenerationArtifactReadRequest,
    ImageGenerationArtifactReadResponse, ImageGenerationArtifactRecoveryDto,
    ImageGenerationCapabilitiesDto, ImageGenerationConfigurationDto,
    ImageGenerationConfigurationMutationOutcomeDto, ImageGenerationConfigurationOperationDto,
    ImageGenerationCredentialMutationDto, ImageGenerationCredentialStatusDto,
    ImageGenerationDefaultsDto, ImageGenerationGetConfigurationResponse,
    ImageGenerationReadinessDto, ImageGenerationSetEnabledRequest, ImageGenerationSizePresetDto,
    ImageGenerationStatusDto, ImageGenerationUpdateConfigurationRequest,
    ImageGenerationUpdateConfigurationResponse, ManagedArtifactReadIdentityDto,
    ManagedDocumentArtifactDto, ManagedDocumentArtifactFormatDto, ManagedDocumentArtifactKindDto,
    IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION, IMAGE_GENERATION_ARTIFACT_ERROR_CODE,
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

#[cfg(test)]
pub(crate) async fn handle_image_generation_artifact_request(
    store: Arc<ManagedImageGenerationArtifactStore>,
    storage: Arc<StorageService>,
    request: JsonRpcRequest,
) -> Value {
    handle_image_generation_artifact_request_with_observer_authority(store, storage, None, request)
        .await
}

pub(crate) async fn handle_image_generation_artifact_request_with_observer_authority(
    store: Arc<ManagedImageGenerationArtifactStore>,
    storage: Arc<StorageService>,
    agent_service: Option<&AgentService>,
    request: JsonRpcRequest,
) -> Value {
    let id = request.id;
    if request.jsonrpc != "2.0" || request.method != IMAGE_GENERATION_READ_ARTIFACT_METHOD {
        return image_generation_artifact_error_response(
            id,
            ImageGenerationArtifactErrorCodeDto::InvalidRequest,
        );
    }
    let input = match parse_params::<ImageGenerationArtifactReadRequest>(request.params) {
        Ok(input) if input.schema_version == IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION => {
            input
        }
        Ok(_) | Err(_) => {
            return image_generation_artifact_error_response(
                id,
                ImageGenerationArtifactErrorCodeDto::InvalidRequest,
            )
        }
    };
    if input.conversation_id.as_deref().is_some_and(|value| {
        value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control)
    }) {
        return image_generation_artifact_error_response(
            id,
            ImageGenerationArtifactErrorCodeDto::InvalidRequest,
        );
    }
    if input
        .observer_root_conversation_id
        .as_deref()
        .is_some_and(|value| {
            value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control)
        })
        || (input.observer_root_conversation_id.is_some() && input.conversation_id.is_none())
    {
        return image_generation_artifact_error_response(
            id,
            ImageGenerationArtifactErrorCodeDto::InvalidRequest,
        );
    }
    if let Some(conversation_id) = input.conversation_id.as_deref() {
        match storage.get_agent_node_by_conversation(conversation_id) {
            Ok(Some(node)) if node.parent_agent_id.is_some() => {
                let authorized = input
                    .observer_root_conversation_id
                    .as_deref()
                    .zip(agent_service)
                    .is_some_and(|(root_conversation_id, service)| {
                        service
                            .authorize_exact_child_observer_read(
                                root_conversation_id,
                                conversation_id,
                            )
                            .is_ok()
                    });
                if !authorized {
                    // Unknown, foreign-tree, and missing observer authority are intentionally
                    // indistinguishable at this byte-bearing boundary.
                    return image_generation_artifact_error_response(
                        id,
                        ImageGenerationArtifactErrorCodeDto::NotFound,
                    );
                }
            }
            Ok(_) if input.observer_root_conversation_id.is_some() => {
                return image_generation_artifact_error_response(
                    id,
                    ImageGenerationArtifactErrorCodeDto::NotFound,
                );
            }
            Ok(_) => {}
            Err(_) => {
                return image_generation_artifact_error_response(
                    id,
                    ImageGenerationArtifactErrorCodeDto::Unavailable,
                );
            }
        }
    }
    let target = match artifact_read_target(&input.artifact) {
        Some(target) => target,
        None => {
            return image_generation_artifact_error_response(
                id,
                ImageGenerationArtifactErrorCodeDto::InvalidRequest,
            )
        }
    };

    let managed_content = if let Some(conversation_id) = input.conversation_id.as_deref() {
        let storage = Arc::clone(&storage);
        let artifact_id = target.artifact_id().to_string();
        let conversation_id = conversation_id.to_string();
        match tokio::task::spawn_blocking(move || {
            storage.read_authorized_managed_artifact(&artifact_id, &conversation_id)
        })
        .await
        {
            Ok(Ok(content)) => content,
            Ok(Err(_)) | Err(_) => {
                return image_generation_artifact_error_response(
                    id,
                    ImageGenerationArtifactErrorCodeDto::Unavailable,
                )
            }
        }
    } else {
        None
    };
    let bytes = if let Some(content) = managed_content {
        let identity_matches = target.matches_managed_content(&content);
        if !identity_matches {
            return image_generation_artifact_error_response(
                id,
                ImageGenerationArtifactErrorCodeDto::IntegrityCheckFailed,
            );
        }
        content.bytes
    } else {
        let ArtifactReadTarget::Image(candidate) = &target else {
            // Generic documents are always exact-conversation or same-Agent-tree grants. A
            // guessed content-addressed URI must never fall through to the generated-image
            // journal.
            return image_generation_artifact_error_response(
                id,
                ImageGenerationArtifactErrorCodeDto::NotFound,
            );
        };
        // The image store is shared by legacy generation and generic managed-command Artifacts.
        // Require the independent publication journal before consulting raw objects so an
        // ungranted generic sha256 URI cannot cross an authorization boundary merely by being
        // guessed.
        let storage_for_lookup = Arc::clone(&storage);
        let artifact_id = candidate.artifact_id.clone();
        let legacy = match tokio::task::spawn_blocking(move || {
            storage_for_lookup.resolve_published_generated_artifact_input(&artifact_id, None)
        })
        .await
        {
            Ok(Ok(legacy)) => legacy,
            Ok(Err(_)) | Err(_) => {
                return image_generation_artifact_error_response(
                    id,
                    ImageGenerationArtifactErrorCodeDto::Unavailable,
                )
            }
        };
        let Some(legacy) = legacy else {
            return image_generation_artifact_error_response(
                id,
                ImageGenerationArtifactErrorCodeDto::NotFound,
            );
        };
        if !matches!(
            legacy.kind,
            mycopilot_core::storage::service::ResolvedGeneratedArtifactKind::ImageGeneration
        ) || legacy.sha256 != candidate.sha256
            || legacy.size_bytes != candidate.size_bytes
        {
            return image_generation_artifact_error_response(
                id,
                ImageGenerationArtifactErrorCodeDto::IntegrityCheckFailed,
            );
        }
        match store.read_published(candidate).await {
            Ok(Some(content)) => content.bytes,
            Ok(None) => {
                return image_generation_artifact_error_response(
                    id,
                    ImageGenerationArtifactErrorCodeDto::NotFound,
                )
            }
            Err(error) => return image_generation_artifact_store_error_response(id, error.code),
        }
    };
    let artifact = target.identity();
    let file_name = target.file_name();
    let data_base64 = match tokio::task::spawn_blocking(move || {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    })
    .await
    {
        Ok(data_base64) => data_base64,
        Err(_) => {
            return image_generation_artifact_error_response(
                id,
                ImageGenerationArtifactErrorCodeDto::Unavailable,
            )
        }
    };
    response_success(
        id,
        ImageGenerationArtifactReadResponse {
            schema_version: IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
            artifact,
            file_name,
            data_base64,
        },
    )
}

enum ArtifactReadTarget {
    Image(ImageGenerationArtifactCandidate),
    Document(ManagedDocumentArtifactDto),
}

impl ArtifactReadTarget {
    fn artifact_id(&self) -> &str {
        match self {
            Self::Image(candidate) => &candidate.artifact_id,
            Self::Document(document) => &document.artifact_id,
        }
    }

    fn identity(&self) -> ManagedArtifactReadIdentityDto {
        match self {
            Self::Image(candidate) => {
                ManagedArtifactReadIdentityDto::Image(artifact_dto(candidate))
            }
            Self::Document(document) => ManagedArtifactReadIdentityDto::Document(document.clone()),
        }
    }

    fn file_name(&self) -> String {
        match self {
            Self::Image(candidate) => format!(
                "generated-image-{}.{}",
                &candidate.sha256[..12],
                candidate.format.extension()
            ),
            Self::Document(document) => format!("artifact-{}.pdf", &document.sha256[..12]),
        }
    }

    fn matches_managed_content(
        &self,
        content: &mycopilot_core::storage::service::AuthorizedManagedArtifactContent,
    ) -> bool {
        use mycopilot_core::storage::managed_artifact_repository::ManagedArtifactKind;
        match self {
            Self::Image(candidate) => {
                content.kind == ManagedArtifactKind::Image
                    && content.format == candidate.format.extension()
                    && content.media_type == candidate.media_type
                    && content.size_bytes == candidate.size_bytes
                    && content.sha256 == candidate.sha256
                    && content.width == Some(candidate.width)
                    && content.height == Some(candidate.height)
            }
            Self::Document(document) => {
                content.kind == ManagedArtifactKind::Document
                    && content.format == "pdf"
                    && content.media_type == "application/pdf"
                    && content.size_bytes == document.size_bytes
                    && content.sha256 == document.sha256
                    && content.width.is_none()
                    && content.height.is_none()
            }
        }
    }
}

fn artifact_read_target(artifact: &ManagedArtifactReadIdentityDto) -> Option<ArtifactReadTarget> {
    match artifact {
        ManagedArtifactReadIdentityDto::Image(artifact) => {
            artifact_candidate(artifact).map(ArtifactReadTarget::Image)
        }
        ManagedArtifactReadIdentityDto::Document(artifact) => {
            let sha256 = artifact.sha256.as_str();
            (is_lower_hex_sha256(sha256)
                && artifact.artifact_id == format!("sha256:{sha256}")
                && artifact.uri == format!("artifact://sha256/{sha256}")
                && artifact.kind == ManagedDocumentArtifactKindDto::Document
                && artifact.format == ManagedDocumentArtifactFormatDto::Pdf
                && artifact.mime_type == "application/pdf"
                && artifact.size_bytes > 0
                && artifact.size_bytes <= 128 * 1024 * 1024)
                .then(|| ArtifactReadTarget::Document(artifact.clone()))
        }
    }
}

fn artifact_candidate(
    artifact: &ImageGenerationArtifactDto,
) -> Option<ImageGenerationArtifactCandidate> {
    let sha256 = artifact.sha256.as_str();
    if !is_lower_hex_sha256(sha256)
        || artifact.artifact_id != format!("sha256:{sha256}")
        || artifact.uri != format!("image-artifact://sha256/{sha256}")
        || artifact.kind != ImageGenerationArtifactKindDto::Image
        || artifact.width == 0
        || artifact.height == 0
        || artifact.width > MAX_IMAGE_ARTIFACT_DIMENSION
        || artifact.height > MAX_IMAGE_ARTIFACT_DIMENSION
        || u64::from(artifact.width)
            .checked_mul(u64::from(artifact.height))
            .is_none_or(|pixels| pixels > MAX_IMAGE_ARTIFACT_PIXELS)
        || artifact.size_bytes == 0
        || artifact.size_bytes > DEFAULT_IMAGE_ARTIFACT_MAX_BYTES as u64
    {
        return None;
    }
    let format = match artifact.format {
        ImageGenerationArtifactFormatDto::Png => ImageArtifactFormat::Png,
        ImageGenerationArtifactFormatDto::Jpeg => ImageArtifactFormat::Jpeg,
        ImageGenerationArtifactFormatDto::Webp => ImageArtifactFormat::Webp,
    };
    if artifact.mime_type != format.media_type() {
        return None;
    }
    let target_name = format!("{sha256}.{}", format.extension());
    Some(ImageGenerationArtifactCandidate {
        artifact_id: artifact.artifact_id.clone(),
        storage_relative_path: format!("objects/{target_name}"),
        format,
        media_type: artifact.mime_type.clone(),
        width: artifact.width,
        height: artifact.height,
        size_bytes: artifact.size_bytes,
        sha256: artifact.sha256.clone(),
    })
}

fn artifact_dto(candidate: &ImageGenerationArtifactCandidate) -> ImageGenerationArtifactDto {
    ImageGenerationArtifactDto {
        artifact_id: candidate.artifact_id.clone(),
        uri: candidate.artifact_uri(),
        kind: ImageGenerationArtifactKindDto::Image,
        format: match candidate.format {
            ImageArtifactFormat::Png => ImageGenerationArtifactFormatDto::Png,
            ImageArtifactFormat::Jpeg => ImageGenerationArtifactFormatDto::Jpeg,
            ImageArtifactFormat::Webp => ImageGenerationArtifactFormatDto::Webp,
        },
        mime_type: candidate.media_type.clone(),
        width: candidate.width,
        height: candidate.height,
        size_bytes: candidate.size_bytes,
        sha256: candidate.sha256.clone(),
    }
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn image_generation_artifact_store_error_response(
    id: JsonRpcId,
    code: ImageArtifactErrorCode,
) -> Value {
    let code = match code {
        ImageArtifactErrorCode::ResponseTooLarge => ImageGenerationArtifactErrorCodeDto::TooLarge,
        ImageArtifactErrorCode::UnsupportedMediaType
        | ImageArtifactErrorCode::InvalidImage
        | ImageArtifactErrorCode::Conflict => {
            ImageGenerationArtifactErrorCodeDto::IntegrityCheckFailed
        }
        ImageArtifactErrorCode::InvalidConfiguration
        | ImageArtifactErrorCode::UnsafeUrl
        | ImageArtifactErrorCode::DnsRejected
        | ImageArtifactErrorCode::RedirectRejected
        | ImageArtifactErrorCode::TransportFailed
        | ImageArtifactErrorCode::DownloadTimedOut
        | ImageArtifactErrorCode::HttpRejected
        | ImageArtifactErrorCode::Io
        | ImageArtifactErrorCode::Cancelled
        | ImageArtifactErrorCode::CommitIndeterminate => {
            ImageGenerationArtifactErrorCodeDto::Unavailable
        }
    };
    image_generation_artifact_error_response(id, code)
}

pub(crate) fn image_generation_artifact_error_response(
    id: JsonRpcId,
    code: ImageGenerationArtifactErrorCodeDto,
) -> Value {
    let (recovery, message, retryable) = match code {
        ImageGenerationArtifactErrorCodeDto::InvalidRequest => (
            ImageGenerationArtifactRecoveryDto::DoNotRetry,
            "The generated image reference is invalid.",
            false,
        ),
        ImageGenerationArtifactErrorCodeDto::NotFound => (
            ImageGenerationArtifactRecoveryDto::Regenerate,
            "The generated image is no longer available.",
            false,
        ),
        ImageGenerationArtifactErrorCodeDto::IntegrityCheckFailed => (
            ImageGenerationArtifactRecoveryDto::Regenerate,
            "The stored image failed its integrity check.",
            false,
        ),
        ImageGenerationArtifactErrorCodeDto::TooLarge => (
            ImageGenerationArtifactRecoveryDto::Regenerate,
            "The stored image exceeds the supported preview size.",
            false,
        ),
        ImageGenerationArtifactErrorCodeDto::Unavailable => (
            ImageGenerationArtifactRecoveryDto::Retry,
            "The generated image could not be read right now.",
            true,
        ),
    };
    let data = ImageGenerationArtifactErrorData {
        error_type: ImageGenerationArtifactErrorTypeDto::ImageGenerationArtifact,
        operation: ImageGenerationArtifactOperationDto::Read,
        code,
        recovery,
        message: message.to_string(),
        retryable,
    };
    serde_json::to_value(error_with_data(
        Some(id),
        IMAGE_GENERATION_ARTIFACT_ERROR_CODE,
        data.message.clone(),
        serde_json::to_value(data).expect("image Artifact error data must serialize"),
    ))
    .expect("JSON-RPC error response must serialize")
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
    debug_assert_eq!(adapter_id, ImageGenerationAdapterId::SmartMlSeedream);
    ImageGenerationAdapterIdDto::SmartMlSeedream
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

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::image_generation::InMemoryCredentialStore;
    use mycopilot_core::storage::image_generation_execution_repository::{
        ImageGenerationArtifactJournalRecord, ImageGenerationExecutionIdentityRecord,
        ImageGenerationExecutionTerminalUpdate, StoredImageGenerationArtifactState,
        StoredImageGenerationExecutionStatus,
    };
    use mycopilot_core::storage::models::{
        ChatConversationMetaRecord, ModelConfigRecord, ModelSettingsRecord,
    };
    use mycopilot_core::{
        AgentForkTurns, CreateChildAgentInput, EnsureRootAgentInput, ProviderProfileConfig,
        ProviderProtocolDialect,
    };
    use sha2::{Digest, Sha256};
    use std::fs;

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

    fn agent_model_settings() -> ModelSettingsRecord {
        ModelSettingsRecord {
            api_url: "https://provider.example/v1/chat/completions".to_string(),
            api_token: "fixture-secret".to_string(),
            search_mode: "disabled".to_string(),
            tavily_api_key: String::new(),
            models: vec![ModelConfigRecord {
                id: "model-a".to_string(),
                display_name: "Model A".to_string(),
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(64_000),
                provider_profile_config: ProviderProfileConfig::generic_for_dialect(
                    ProviderProtocolDialect::OpenAiChatCompletions,
                ),
                input_price: "0".to_string(),
                cached_input_price: String::new(),
                output_price: "0".to_string(),
                enabled: true,
            }],
        }
    }

    fn artifact_fixture() -> (
        tempfile::TempDir,
        Arc<ManagedImageGenerationArtifactStore>,
        Arc<StorageService>,
        Vec<u8>,
        Value,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            ManagedImageGenerationArtifactStore::new(
                temp.path().join("image-generation-artifacts"),
                ImageArtifactStoreConfig::default(),
            )
            .unwrap(),
        );
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
            )
            .unwrap();
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        fs::write(
            temp.path()
                .join("image-generation-artifacts/objects")
                .join(format!("{sha256}.png")),
            &bytes,
        )
        .unwrap();
        let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
        let identity = ImageGenerationExecutionIdentityRecord {
            execution_id: "legacy-execution-1".to_string(),
            request_fingerprint: format!("sha256:{}", "a".repeat(64)),
            safe_request_json: r#"{"schemaVersion":1}"#.to_string(),
            profile_id: "default".to_string(),
            adapter_id: "smartmlSeedream".to_string(),
            profile_revision: 1,
            model_id: "image-model".to_string(),
            operation: "generate".to_string(),
        };
        storage.claim_image_generation_execution(&identity).unwrap();
        storage
            .prepare_image_generation_artifact(
                &identity.execution_id,
                &ImageGenerationArtifactJournalRecord {
                    ordinal: 0,
                    artifact_id: format!("sha256:{sha256}"),
                    state: StoredImageGenerationArtifactState::Candidate,
                    storage_relative_path: format!("objects/{sha256}.png"),
                    format: "png".to_string(),
                    media_type: "image/png".to_string(),
                    width: 1,
                    height: 1,
                    size_bytes: bytes.len() as u64,
                    sha256: sha256.clone(),
                    created_at: 1,
                    published_at: None,
                },
                Some("provider-request"),
                Some(200),
            )
            .unwrap();
        storage
            .finalize_image_generation_execution(
                &identity.execution_id,
                &ImageGenerationExecutionTerminalUpdate {
                    expected_request_fingerprint: identity.request_fingerprint,
                    expected_artifact_sha256: Some(sha256.clone()),
                    status: StoredImageGenerationExecutionStatus::Succeeded,
                    remote_outcome_unknown: false,
                    provider_succeeded: true,
                    commit_may_have_succeeded: false,
                    provider_request_id: Some("provider-request".to_string()),
                    http_status: Some(200),
                    terminal_result_json: r#"{"schemaVersion":1,"status":"succeeded"}"#.to_string(),
                },
            )
            .unwrap();
        let artifact = json!({
            "artifactId": format!("sha256:{sha256}"),
            "uri": format!("image-artifact://sha256/{sha256}"),
            "kind": "image",
            "format": "png",
            "mimeType": "image/png",
            "width": 1,
            "height": 1,
            "sizeBytes": bytes.len(),
            "sha256": sha256,
        });
        (temp, store, storage, bytes, artifact)
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

    #[tokio::test]
    async fn historical_artifact_read_is_independent_of_current_provider_configuration() {
        let (_temp, store, storage, bytes, artifact) = artifact_fixture();
        let response = handle_image_generation_artifact_request(
            store,
            storage,
            JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: JsonRpcId::Number(1),
                method: IMAGE_GENERATION_READ_ARTIFACT_METHOD.to_string(),
                params: Some(json!({
                    "schemaVersion": IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
                    "artifact": artifact.clone(),
                })),
            },
        )
        .await;

        assert_eq!(
            response["result"]["dataBase64"],
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        assert_eq!(response["result"]["artifact"], artifact);
        assert!(response.to_string().contains("generated-image-"));
        assert!(!response.to_string().contains("provider"));
        assert!(!response.to_string().contains("image-generation-artifacts"));
    }

    #[tokio::test]
    async fn artifact_read_returns_stable_not_found_and_identity_errors() {
        let (_temp, store, storage, _bytes, artifact) = artifact_fixture();
        let mut mismatched = artifact.clone();
        mismatched["width"] = json!(2);
        let invalid = handle_image_generation_artifact_request(
            Arc::clone(&store),
            Arc::clone(&storage),
            JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: JsonRpcId::Number(1),
                method: IMAGE_GENERATION_READ_ARTIFACT_METHOD.to_string(),
                params: Some(json!({
                    "schemaVersion": IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
                    "artifact": mismatched,
                })),
            },
        )
        .await;
        assert_eq!(
            invalid["error"]["code"],
            IMAGE_GENERATION_ARTIFACT_ERROR_CODE
        );
        assert_eq!(invalid["error"]["data"]["code"], "integrityCheckFailed");

        let missing_sha = "f".repeat(64);
        let missing = handle_image_generation_artifact_request(
            store,
            storage,
            JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: JsonRpcId::Number(2),
                method: IMAGE_GENERATION_READ_ARTIFACT_METHOD.to_string(),
                params: Some(json!({
                    "schemaVersion": IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
                    "artifact": {
                        "artifactId": format!("sha256:{missing_sha}"),
                        "uri": format!("image-artifact://sha256/{missing_sha}"),
                        "kind": "image",
                        "format": "png",
                        "mimeType": "image/png",
                        "width": 1,
                        "height": 1,
                        "sizeBytes": 1,
                        "sha256": missing_sha,
                    },
                })),
            },
        )
        .await;
        assert_eq!(missing["error"]["data"]["code"], "notFound");
        assert_eq!(missing["error"]["data"]["recovery"], "regenerate");
    }

    #[tokio::test]
    async fn managed_command_image_read_accepts_same_tree_grants_but_keeps_observer_authority() {
        let temp = tempfile::tempdir().unwrap();
        let database_path = temp.path().join("storage.sqlite");
        let storage = Arc::new(StorageService::open(&database_path).unwrap());
        storage.save_model_settings(agent_model_settings()).unwrap();
        for conversation_id in ["conversation-root", "conversation-unbound"] {
            storage
                .save_conversation_meta(ChatConversationMetaRecord {
                    id: conversation_id.to_string(),
                    project_id: None,
                    model_id: Some("model-a".to_string()),
                    title: conversation_id.to_string(),
                    created_at: 1,
                    updated_at: 1,
                    pinned_at: None,
                    archived_at: None,
                    unread_at: None,
                })
                .unwrap();
        }
        storage
            .ensure_root_agent(&EnsureRootAgentInput {
                agent_id: "agent-root".to_string(),
                conversation_id: "conversation-root".to_string(),
                creation_request_id: "ensure-root".to_string(),
                task_name: "Root".to_string(),
            })
            .unwrap();
        let child = storage
            .create_child_agent(&CreateChildAgentInput {
                parent_agent_id: "agent-root".to_string(),
                creation_request_id: "spawn-child".to_string(),
                task_name: "artifact-child".to_string(),
                task: "Create a managed image.".to_string(),
                template_machine_key: None,
                explicit_model_id: None,
                reasoning_effort: None,
                fork_turns: AgentForkTurns::None,
            })
            .unwrap();
        let child_conversation_id = child.agent.conversation_id;
        let store = Arc::new(
            ManagedImageGenerationArtifactStore::new(
                temp.path().join("image-generation-artifacts"),
                ImageArtifactStoreConfig::default(),
            )
            .unwrap(),
        );
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
            )
            .unwrap();
        let source = temp.path().join("page.png");
        fs::write(&source, &bytes).unwrap();
        let published = storage
            .publish_managed_artifact_file(
                &source,
                mycopilot_core::storage::service::ManagedArtifactAuthority {
                    conversation_id: "conversation-root",
                    run_id: "run-root",
                    call_id: "call-root",
                },
            )
            .unwrap();
        storage
            .publish_managed_artifact_file(
                &source,
                mycopilot_core::storage::service::ManagedArtifactAuthority {
                    conversation_id: "conversation-unbound",
                    run_id: "run-unbound",
                    call_id: "call-unbound",
                },
            )
            .unwrap();
        let artifact = json!({
            "artifactId": format!("sha256:{}", published.sha256),
            "uri": published.read_path(),
            "kind": "image",
            "format": "png",
            "mimeType": "image/png",
            "width": 1,
            "height": 1,
            "sizeBytes": published.size_bytes,
            "sha256": published.sha256,
        });
        let request = |id, conversation_id: Option<&str>| JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: JsonRpcId::Number(id),
            method: IMAGE_GENERATION_READ_ARTIFACT_METHOD.to_string(),
            params: Some(json!({
                "schemaVersion": IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
                "artifact": artifact,
                "conversationId": conversation_id,
            })),
        };

        for (id, conversation_id) in [(1, "conversation-root"), (2, "conversation-unbound")] {
            let authorized = handle_image_generation_artifact_request(
                Arc::clone(&store),
                Arc::clone(&storage),
                request(id, Some(conversation_id)),
            )
            .await;
            assert_eq!(
                authorized["result"]["dataBase64"],
                base64::engine::general_purpose::STANDARD.encode(&bytes)
            );
        }

        let agent_service = AgentService::new(Arc::clone(&storage));
        let observer_authorized = handle_image_generation_artifact_request_with_observer_authority(
            Arc::clone(&store),
            Arc::clone(&storage),
            Some(&agent_service),
            JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: JsonRpcId::Number(20),
                method: IMAGE_GENERATION_READ_ARTIFACT_METHOD.to_string(),
                params: Some(json!({
                    "schemaVersion": IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
                    "artifact": artifact.clone(),
                    "conversationId": child_conversation_id,
                    "observerRootConversationId": "conversation-root",
                })),
            },
        )
        .await;
        assert_eq!(
            observer_authorized["result"]["dataBase64"],
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        );

        let observer_cross_tree = handle_image_generation_artifact_request_with_observer_authority(
            Arc::clone(&store),
            Arc::clone(&storage),
            Some(&agent_service),
            JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: JsonRpcId::Number(21),
                method: IMAGE_GENERATION_READ_ARTIFACT_METHOD.to_string(),
                params: Some(json!({
                    "schemaVersion": IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
                    "artifact": artifact.clone(),
                    "conversationId": child_conversation_id,
                    "observerRootConversationId": "conversation-unbound",
                })),
            },
        )
        .await;
        assert_eq!(observer_cross_tree["error"]["data"]["code"], "notFound");

        for (id, conversation_id) in [
            (3, Some(child_conversation_id.as_str())),
            (4, Some("conversation-other")),
            (5, None),
        ] {
            let rejected = handle_image_generation_artifact_request(
                Arc::clone(&store),
                Arc::clone(&storage),
                request(id, conversation_id),
            )
            .await;
            assert_eq!(rejected["error"]["data"]["code"], "notFound");
        }
    }

    #[tokio::test]
    async fn managed_pdf_read_requires_grant_and_exact_identity() {
        let temp = tempfile::tempdir().unwrap();
        let database_path = temp.path().join("storage.sqlite");
        let storage = Arc::new(StorageService::open(&database_path).unwrap());
        let connection = rusqlite::Connection::open(&database_path).unwrap();
        connection
            .execute(
                "INSERT INTO conversations (id, title, created_at, updated_at) VALUES (?1, ?2, 1, 1)",
                ["conversation-pdf", "test"],
            )
            .unwrap();
        let store = Arc::new(
            ManagedImageGenerationArtifactStore::new(
                temp.path().join("image-generation-artifacts"),
                ImageArtifactStoreConfig::default(),
            )
            .unwrap(),
        );
        let bytes = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\ntrailer\n<<>>\n%%EOF\n".to_vec();
        let source = temp.path().join("report.pdf");
        fs::write(&source, &bytes).unwrap();
        let published = storage
            .publish_managed_artifact_file(
                &source,
                mycopilot_core::storage::service::ManagedArtifactAuthority {
                    conversation_id: "conversation-pdf",
                    run_id: "run-pdf",
                    call_id: "call-pdf",
                },
            )
            .unwrap();
        let artifact = json!({
            "artifactId": format!("sha256:{}", published.sha256),
            "uri": published.read_path(),
            "kind": "document",
            "format": "pdf",
            "mimeType": "application/pdf",
            "sizeBytes": published.size_bytes,
            "sha256": published.sha256,
        });
        let request = |id, conversation_id: &str, artifact: Value| JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: JsonRpcId::Number(id),
            method: IMAGE_GENERATION_READ_ARTIFACT_METHOD.to_string(),
            params: Some(json!({
                "schemaVersion": IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
                "artifact": artifact,
                "conversationId": conversation_id,
            })),
        };

        let authorized = handle_image_generation_artifact_request(
            Arc::clone(&store),
            Arc::clone(&storage),
            request(1, "conversation-pdf", artifact.clone()),
        )
        .await;
        assert_eq!(
            authorized["result"]["dataBase64"],
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        );
        assert!(authorized["result"]["fileName"]
            .as_str()
            .unwrap()
            .ends_with(".pdf"));

        let cross_conversation = handle_image_generation_artifact_request(
            Arc::clone(&store),
            Arc::clone(&storage),
            request(2, "conversation-other", artifact.clone()),
        )
        .await;
        assert_eq!(cross_conversation["error"]["data"]["code"], "notFound");

        let mut tampered = artifact;
        tampered["sizeBytes"] = json!(published.size_bytes + 1);
        let identity_error = handle_image_generation_artifact_request(
            store,
            storage,
            request(3, "conversation-pdf", tampered),
        )
        .await;
        assert_eq!(
            identity_error["error"]["data"]["code"],
            "integrityCheckFailed"
        );
    }
}
