use super::credential_store::CredentialSecret;
use super::http::{
    ImageHttpClient, ImageHttpClientConfig, ImageHttpEndpointPolicy, ImageHttpResponse,
};
use super::provider::{ImageGenerationProvider, ImageGenerationProviderFactory};
use super::types::{
    ImageGenerationAdapterId, ImageGenerationError, ImageGenerationErrorCode,
    ImageGenerationOperation, ImageGenerationProviderProfile, ImageGenerationRequest,
    ImageGenerationResult, ImageGenerationResultStatus, ImageGenerationUrlOutput,
    PreparedImageGenerationRequest,
};
use super::{ImageArtifactHostRule, ImageArtifactTransferPolicy};
use futures_util::future::BoxFuture;
use reqwest::Url;
use serde_json::{json, Map, Value};

const SEEDREAM_SEQUENTIAL_IMAGE_GENERATION: &str = "disabled";
const SEEDREAM_RESPONSE_FORMAT: &str = "url";
const SEEDREAM_TRUSTED_FAKE_IP_ARTIFACT_HOSTS: &[ImageArtifactHostRule] =
    &[ImageArtifactHostRule::ExactHttpsOrigin {
        host: "ark-content-generation-v2-cn-beijing.tos-cn-beijing.volces.com",
        port: 443,
    }];

/// SmartML adapter for the Seedream-compatible `/images/generations` contract.
///
/// The model-facing request cannot select a provider, endpoint, model, bearer token, response
/// transport, streaming behavior, sequential generation behavior, or vendor-specific raw field.
/// Those values are resolved from the frozen host profile and this adapter's fixed mapping.
pub struct SmartMlSeedreamProvider {
    profile: ImageGenerationProviderProfile,
    endpoint: Url,
    http: ImageHttpClient,
}

/// Production factory registered for the `smartmlSeedream` adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct SmartMlSeedreamProviderFactory {
    http_config: ImageHttpClientConfig,
}

impl SmartMlSeedreamProviderFactory {
    #[must_use]
    pub fn new(http_config: ImageHttpClientConfig) -> Self {
        Self { http_config }
    }
}

impl ImageGenerationProviderFactory for SmartMlSeedreamProviderFactory {
    fn adapter_id(&self) -> ImageGenerationAdapterId {
        ImageGenerationAdapterId::SmartMlSeedream
    }

    fn create(
        &self,
        profile: ImageGenerationProviderProfile,
    ) -> Result<std::sync::Arc<dyn ImageGenerationProvider>, ImageGenerationError> {
        SmartMlSeedreamProvider::new(profile, self.http_config).map(|provider| {
            std::sync::Arc::new(provider) as std::sync::Arc<dyn ImageGenerationProvider>
        })
    }
}

impl SmartMlSeedreamProvider {
    pub fn new(
        profile: ImageGenerationProviderProfile,
        http_config: ImageHttpClientConfig,
    ) -> Result<Self, ImageGenerationError> {
        profile.validate()?;
        if profile.adapter_id != ImageGenerationAdapterId::SmartMlSeedream {
            return Err(ImageGenerationError::invalid_configuration(
                "SmartML Seedream provider requires the smartmlSeedream adapter id",
            ));
        }
        let http = ImageHttpClient::new(http_config)?;
        let endpoint = http.validate_endpoint(&profile.endpoint_url)?;
        Ok(Self {
            profile,
            endpoint,
            http,
        })
    }

    #[must_use]
    pub fn http_config(&self) -> ImageHttpClientConfig {
        self.http.config()
    }

    fn validate_prepared(
        &self,
        request: &PreparedImageGenerationRequest,
    ) -> Result<(), ImageGenerationError> {
        if request.provider_profile_id() != self.profile.id
            || request.adapter_id() != &self.profile.adapter_id
            || request.profile_revision() != self.profile.revision
            || request.endpoint_url() != self.profile.endpoint_url
            || request.model_id() != self.profile.model_id
        {
            return Err(ImageGenerationError::profile_conflict(
                "prepared image-generation request no longer matches the active provider profile",
            ));
        }
        Ok(())
    }
}

impl ImageGenerationProvider for SmartMlSeedreamProvider {
    fn profile(&self) -> &ImageGenerationProviderProfile {
        &self.profile
    }

    fn artifact_transfer_policy(&self) -> ImageArtifactTransferPolicy {
        // The configured endpoint is an explicit user trust decision. Known Adapter CDN origins
        // are separately reviewed. Any other signed CDN URL must pass independent public-DNS
        // verification in the Artifact layer instead of receiving a direct Fake-IP exception.
        ImageArtifactTransferPolicy::with_trusted_fake_ip_https_hosts(
            SEEDREAM_TRUSTED_FAKE_IP_ARTIFACT_HOSTS,
        )
        .with_configured_endpoint_https_origin(
            self.endpoint
                .host_str()
                .expect("validated image-generation endpoint has a host"),
            self.endpoint
                .port_or_known_default()
                .expect("validated HTTPS endpoint has a port"),
        )
    }

    fn prepare(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<PreparedImageGenerationRequest, ImageGenerationError> {
        let request = request.normalize(&self.profile)?;
        Ok(PreparedImageGenerationRequest::new(&self.profile, request))
    }

    fn execute<'a>(
        &'a self,
        request: &'a PreparedImageGenerationRequest,
        credential: &'a CredentialSecret,
    ) -> BoxFuture<'a, Result<ImageGenerationResult, ImageGenerationError>> {
        Box::pin(async move {
            self.validate_prepared(request)?;
            if request.normalized().operation() == ImageGenerationOperation::Status {
                return Ok(ImageGenerationResult::ready(&self.profile));
            }

            let payload = seedream_payload(request)?;
            let response = self
                .http
                .post_json(self.endpoint.clone(), credential, &payload)
                .await?;
            map_seedream_response(
                &self.profile,
                request,
                response,
                self.http.config().endpoint_policy,
            )
        })
    }
}

fn seedream_payload(
    request: &PreparedImageGenerationRequest,
) -> Result<Value, ImageGenerationError> {
    let normalized = request.normalized();
    if normalized.output_count() != 1 {
        return Err(ImageGenerationError::invalid_request(
            "SmartML Seedream supports exactly one output image in this contract",
        ));
    }
    let prompt = normalized.prompt().ok_or_else(|| {
        ImageGenerationError::invalid_request(
            "generate and edit operations require a normalized prompt",
        )
    })?;
    let mut payload = Map::from_iter([
        ("model".to_string(), json!(request.model_id())),
        ("prompt".to_string(), json!(prompt)),
        (
            "sequential_image_generation".to_string(),
            json!(SEEDREAM_SEQUENTIAL_IMAGE_GENERATION),
        ),
        (
            "response_format".to_string(),
            json!(SEEDREAM_RESPONSE_FORMAT),
        ),
        (
            "size".to_string(),
            json!(normalized.size_preset().as_provider_value()),
        ),
        ("stream".to_string(), json!(false)),
        ("watermark".to_string(), json!(normalized.watermark())),
    ]);

    match normalized.operation() {
        ImageGenerationOperation::Generate => {
            if !normalized.inputs().is_empty() {
                return Err(ImageGenerationError::invalid_request(
                    "text-to-image must not contain image inputs",
                ));
            }
        }
        ImageGenerationOperation::Edit => {
            let [input] = normalized.inputs() else {
                return Err(ImageGenerationError::invalid_request(
                    "image-to-image requires exactly one data URL input",
                ));
            };
            payload.insert("image".to_string(), json!(input.data_url()));
        }
        ImageGenerationOperation::Status => {
            return Err(ImageGenerationError::invalid_request(
                "status does not produce a SmartML Seedream HTTP payload",
            ));
        }
    }

    Ok(Value::Object(payload))
}

fn map_seedream_response(
    profile: &ImageGenerationProviderProfile,
    request: &PreparedImageGenerationRequest,
    response: ImageHttpResponse,
    endpoint_policy: ImageHttpEndpointPolicy,
) -> Result<ImageGenerationResult, ImageGenerationError> {
    if !response.is_success() {
        return Err(map_seedream_http_error(&response));
    }
    let status = response.status();
    let provider_request_id = response.provider_request_id().map(ToString::to_string);
    let body = response.json()?;
    let data = body.get("data").and_then(Value::as_array).ok_or_else(|| {
        invalid_response_error(
            status,
            provider_request_id.clone(),
            "image-generation provider response is missing its data array",
        )
    })?;
    let [item] = data.as_slice() else {
        return Err(invalid_response_error(
            status,
            provider_request_id,
            "image-generation provider must return exactly one output",
        ));
    };
    let url = item.get("url").and_then(Value::as_str).ok_or_else(|| {
        invalid_response_error(
            status,
            provider_request_id.clone(),
            "image-generation provider output is missing its URL",
        )
    })?;
    let url = validate_output_url(url, endpoint_policy).map_err(|mut error| {
        error.http_status = Some(status);
        error.provider_request_id = provider_request_id.clone();
        error
    })?;

    Ok(ImageGenerationResult {
        status: ImageGenerationResultStatus::Succeeded,
        provider_profile_id: profile.id.clone(),
        adapter_id: profile.adapter_id.clone(),
        profile_revision: profile.revision,
        model_id: profile.model_id.clone(),
        operation: request.normalized().operation(),
        http_status: Some(status),
        provider_request_id,
        outputs: vec![ImageGenerationUrlOutput::new(url.to_string())],
        capabilities: None,
    })
}

fn map_seedream_http_error(response: &ImageHttpResponse) -> ImageGenerationError {
    let status = response.status();
    let provider_code = response.json().ok().as_ref().and_then(provider_error_code);
    let (code, message, retryable) = match status {
        401 | 403 => (
            ImageGenerationErrorCode::AuthenticationFailed,
            "image-generation provider rejected the configured credential",
            false,
        ),
        408 => (
            ImageGenerationErrorCode::RequestTimedOut,
            "image-generation provider timed out while processing the request",
            false,
        ),
        429 => (
            ImageGenerationErrorCode::RateLimited,
            "image-generation provider rate limit was exceeded",
            true,
        ),
        400 | 404 | 409 | 422 => (
            ImageGenerationErrorCode::ProviderRejected,
            "image-generation provider rejected the normalized request",
            false,
        ),
        500..=599 => (
            ImageGenerationErrorCode::ProviderUnavailable,
            "image-generation provider is temporarily unavailable",
            true,
        ),
        300..=399 => (
            ImageGenerationErrorCode::ProviderUnavailable,
            "image-generation provider attempted an unsupported redirect",
            false,
        ),
        _ => (
            ImageGenerationErrorCode::ProviderRejected,
            "image-generation provider returned an unsuccessful status",
            false,
        ),
    };
    ImageGenerationError::new(code, message, retryable)
        .with_http_status(status)
        .with_provider_code(provider_code)
        .with_provider_request_id(response.provider_request_id().map(ToString::to_string))
        .with_retry_after_seconds(response.retry_after_seconds())
}

fn provider_error_code(body: &Value) -> Option<String> {
    let candidate = body
        .pointer("/error/code")
        .or_else(|| body.get("code"))
        .and_then(Value::as_str)?;
    // Never copy arbitrary provider text into diagnostics. These canonical labels are a small,
    // vendor-independent allowlist and cannot be used to echo a credential-shaped value.
    match candidate.trim().to_ascii_lowercase().as_str() {
        "invalid_api_key" | "authentication_failed" => Some("authentication_failed".to_string()),
        "rate_limit" | "rate_limit_exceeded" => Some("rate_limit".to_string()),
        "content_policy_violation" | "content_filter" => {
            Some("content_policy_violation".to_string())
        }
        "invalid_request" | "invalid_argument" => Some("invalid_request".to_string()),
        "model_not_found" => Some("model_not_found".to_string()),
        "server_error" | "internal_error" => Some("server_error".to_string()),
        _ => None,
    }
}

fn validate_output_url(
    value: &str,
    endpoint_policy: ImageHttpEndpointPolicy,
) -> Result<Url, ImageGenerationError> {
    let url = Url::parse(value).map_err(|_| {
        ImageGenerationError::new(
            ImageGenerationErrorCode::InvalidResponse,
            "image-generation provider returned an invalid output URL",
            false,
        )
    })?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(ImageGenerationError::new(
            ImageGenerationErrorCode::InvalidResponse,
            "image-generation provider returned an unsafe output URL",
            false,
        ));
    }
    let host = url.host_str().ok_or_else(|| {
        ImageGenerationError::new(
            ImageGenerationErrorCode::InvalidResponse,
            "image-generation provider output URL has no host",
            false,
        )
    })?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if url.scheme() != "https"
        && !(url.scheme() == "http"
            && endpoint_policy == ImageHttpEndpointPolicy::AllowLoopbackHttpForTests
            && loopback)
    {
        return Err(ImageGenerationError::new(
            ImageGenerationErrorCode::InvalidResponse,
            "image-generation provider output URL must use HTTPS",
            false,
        ));
    }
    Ok(url)
}

fn invalid_response_error(
    status: u16,
    provider_request_id: Option<String>,
    message: &'static str,
) -> ImageGenerationError {
    ImageGenerationError::new(ImageGenerationErrorCode::InvalidResponse, message, false)
        .with_http_status(status)
        .with_provider_request_id(provider_request_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_generation::types::{
        ImageGenerationCapabilities, ImageGenerationDataUrlInput, ImageGenerationDefaults,
        ImageGenerationEditRequest, ImageGenerationGenerateRequest,
    };
    use base64::Engine;

    fn profile(image_to_image: bool) -> ImageGenerationProviderProfile {
        ImageGenerationProviderProfile::new(
            "default",
            ImageGenerationAdapterId::SmartMlSeedream,
            "https://zju.smartml.cn/userapi/v1/images/generations",
            "doubao-seedream-4-0-250828",
            7,
            ImageGenerationCapabilities::smartml_seedream(image_to_image),
            ImageGenerationDefaults::default(),
        )
        .unwrap()
    }

    fn provider(image_to_image: bool) -> SmartMlSeedreamProvider {
        SmartMlSeedreamProvider::new(profile(image_to_image), ImageHttpClientConfig::default())
            .unwrap()
    }

    fn png_data_url() -> String {
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\ncontent")
        )
    }

    #[test]
    fn text_to_image_payload_freezes_vendor_fields_and_defaults() {
        let provider = provider(false);
        let prepared = provider
            .prepare(ImageGenerationRequest::Generate(
                ImageGenerationGenerateRequest::new("interstellar train"),
            ))
            .unwrap();
        let payload = seedream_payload(&prepared).unwrap();

        assert_eq!(payload["model"], "doubao-seedream-4-0-250828");
        assert_eq!(payload["prompt"], "interstellar train");
        assert_eq!(payload["sequential_image_generation"], "disabled");
        assert_eq!(payload["response_format"], "url");
        assert_eq!(payload["size"], "2K");
        assert_eq!(payload["stream"], false);
        assert_eq!(payload["watermark"], true);
        assert!(payload.get("image").is_none());
    }

    #[test]
    fn image_to_image_payload_requires_enabled_capability() {
        let input = ImageGenerationDataUrlInput::parse(png_data_url()).unwrap();
        let request = ImageGenerationRequest::Edit(ImageGenerationEditRequest::new(
            "replace the background",
            input,
        ));
        assert_eq!(
            provider(false).prepare(request.clone()).unwrap_err().code,
            ImageGenerationErrorCode::UnsupportedOperation
        );

        let prepared = provider(true).prepare(request).unwrap();
        let payload = seedream_payload(&prepared).unwrap();
        assert_eq!(payload["image"], png_data_url());
    }

    #[test]
    fn success_mapping_requires_exactly_one_https_url() {
        let provider = provider(false);
        let prepared = provider
            .prepare(ImageGenerationRequest::Generate(
                ImageGenerationGenerateRequest::new("a train"),
            ))
            .unwrap();
        let response = ImageHttpResponse::from_test_parts(
            200,
            Some("request-1"),
            None,
            br#"{"data":[{"url":"https://cdn.example/result.png?signature=secret"}]}"#.to_vec(),
        );
        let result = map_seedream_response(
            provider.profile(),
            &prepared,
            response,
            ImageHttpEndpointPolicy::HttpsOnly,
        )
        .unwrap();

        assert_eq!(result.status, ImageGenerationResultStatus::Succeeded);
        assert_eq!(result.outputs.len(), 1);
        assert!(result
            .provider_request_id
            .as_deref()
            .is_some_and(|value| value.starts_with("sha256:")));
        assert!(format!("{:?}", result.outputs[0]).contains("[REDACTED]"));
        assert!(!format!("{:?}", result.outputs[0]).contains("signature"));
    }

    #[test]
    fn adapter_grants_fake_ip_compatibility_only_to_configured_and_reviewed_origins() {
        let policy = provider(false).artifact_transfer_policy();

        assert!(policy.permits_fake_ip_for_origin(
            "ark-content-generation-v2-cn-beijing.tos-cn-beijing.volces.com",
            443,
        ));
        assert!(policy.permits_fake_ip_for_origin("zju.smartml.cn", 443));
        assert!(!policy.permits_fake_ip_for_origin("zju.smartml.cn", 8443));
        assert!(!policy.permits_fake_ip_for_origin("images.another-provider.com", 443));
        assert!(!policy.permits_fake_ip_for_origin("localhost", 443));
        assert!(!policy.permits_fake_ip_for_origin("images.local", 443));
        assert!(!policy.permits_fake_ip_for_origin("intranet", 443));
        assert!(!policy.permits_fake_ip_for_origin("198.18.0.8", 443));
    }

    #[test]
    fn provider_error_mapping_never_echoes_upstream_body() {
        let response = ImageHttpResponse::from_test_parts(
            401,
            Some("request-2"),
            None,
            br#"{"error":{"code":"invalid_api_key","message":"Bearer never-print-this"}}"#.to_vec(),
        );
        let error = map_seedream_http_error(&response);
        let debug = format!("{error:?}");

        assert_eq!(error.code, ImageGenerationErrorCode::AuthenticationFailed);
        assert_eq!(
            error.provider_code.as_deref(),
            Some("authentication_failed")
        );
        assert!(!debug.contains("never-print-this"));
        assert!(!debug.contains("Bearer"));
    }

    #[test]
    fn rate_limit_preserves_safe_retry_metadata() {
        let response = ImageHttpResponse::from_test_parts(
            429,
            Some("request-3"),
            Some(30),
            br#"{"error":{"code":"rate_limit"}}"#.to_vec(),
        );
        let error = map_seedream_http_error(&response);
        assert_eq!(error.code, ImageGenerationErrorCode::RateLimited);
        assert!(error.retryable);
        assert_eq!(error.retry_after_seconds, Some(30));
    }

    #[test]
    fn unknown_provider_identity_fields_cannot_smuggle_secret_shaped_values() {
        let token = "sk_live_abcdefghijklmnopqrstuvwxyz";
        let response = ImageHttpResponse::from_test_parts(
            400,
            Some(token),
            None,
            format!(r#"{{"error":{{"code":"{token}"}},"request_id":"{token}"}}"#).into_bytes(),
        );

        let error = map_seedream_http_error(&response);
        let debug = format!("{error:?}");

        assert!(error.provider_code.is_none());
        assert!(error
            .provider_request_id
            .as_deref()
            .is_some_and(|value| value.starts_with("sha256:")));
        assert!(!debug.contains(token));
    }

    #[test]
    fn prepared_profile_revision_mismatch_fails_before_network() {
        let first = provider(false);
        let prepared = first
            .prepare(ImageGenerationRequest::Generate(
                ImageGenerationGenerateRequest::new("a train"),
            ))
            .unwrap();
        let mut changed_profile = profile(false);
        changed_profile.revision += 1;
        let changed =
            SmartMlSeedreamProvider::new(changed_profile, ImageHttpClientConfig::default())
                .unwrap();
        assert_eq!(
            changed.validate_prepared(&prepared).unwrap_err().code,
            ImageGenerationErrorCode::ProfileConflict
        );
    }
}
