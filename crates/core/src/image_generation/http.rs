use super::credential_store::CredentialSecret;
use super::types::{ImageGenerationError, ImageGenerationErrorCode};
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER};
use reqwest::{Client, Url};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fmt::Write as _;
use std::net::IpAddr;
use std::time::Duration;
use zeroize::Zeroize;

pub const DEFAULT_IMAGE_HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_IMAGE_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
pub const DEFAULT_IMAGE_HTTP_MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_CONFIGURED_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_CONFIGURED_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageHttpEndpointPolicy {
    HttpsOnly,
    /// Test-only escape hatch. HTTP remains limited to a literal loopback address or localhost.
    AllowLoopbackHttpForTests,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageHttpClientConfig {
    pub endpoint_policy: ImageHttpEndpointPolicy,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub max_response_bytes: usize,
}

impl Default for ImageHttpClientConfig {
    fn default() -> Self {
        Self {
            endpoint_policy: ImageHttpEndpointPolicy::HttpsOnly,
            connect_timeout: DEFAULT_IMAGE_HTTP_CONNECT_TIMEOUT,
            request_timeout: DEFAULT_IMAGE_HTTP_REQUEST_TIMEOUT,
            max_response_bytes: DEFAULT_IMAGE_HTTP_MAX_RESPONSE_BYTES,
        }
    }
}

impl ImageHttpClientConfig {
    fn validate(self) -> Result<Self, ImageGenerationError> {
        if self.connect_timeout.is_zero()
            || self.request_timeout.is_zero()
            || self.connect_timeout > self.request_timeout
            || self.request_timeout > MAX_CONFIGURED_TIMEOUT
        {
            return Err(ImageGenerationError::invalid_configuration(
                "image-generation HTTP timeouts are invalid",
            ));
        }
        if self.max_response_bytes == 0 || self.max_response_bytes > MAX_CONFIGURED_RESPONSE_BYTES {
            return Err(ImageGenerationError::invalid_configuration(
                "image-generation HTTP response limit is invalid",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone)]
pub struct ImageHttpClient {
    client: Client,
    config: ImageHttpClientConfig,
}

impl ImageHttpClient {
    pub fn new(config: ImageHttpClientConfig) -> Result<Self, ImageGenerationError> {
        let config = config.validate()?;
        let client = Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            // Redirects are deliberately disabled. In particular, a configured bearer token is
            // never forwarded to an origin selected by an upstream redirect response.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| {
                ImageGenerationError::invalid_configuration(
                    "image-generation HTTP client could not be initialized",
                )
            })?;
        Ok(Self { client, config })
    }

    #[must_use]
    pub fn config(&self) -> ImageHttpClientConfig {
        self.config
    }

    pub fn validate_endpoint(&self, value: &str) -> Result<Url, ImageGenerationError> {
        validate_endpoint(value, self.config.endpoint_policy)
    }

    pub async fn post_json(
        &self,
        endpoint: Url,
        credential: &CredentialSecret,
        payload: &Value,
    ) -> Result<ImageHttpResponse, ImageGenerationError> {
        // Revalidate at execution rather than trusting the constructor-time snapshot alone.
        let endpoint = validate_endpoint(endpoint.as_str(), self.config.endpoint_policy)?;
        let authorization = bearer_header(credential)?;
        let response = self
            .client
            .post(endpoint)
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .header(AUTHORIZATION, authorization)
            .json(payload)
            .send()
            .await
            .map_err(classify_transport_error)?;

        let status = response.status().as_u16();
        let headers = response.headers().clone();
        if response
            .content_length()
            .is_some_and(|length| length > self.config.max_response_bytes as u64)
        {
            return Err(ImageGenerationError::new(
                ImageGenerationErrorCode::ResponseTooLarge,
                "image-generation provider response exceeds the configured byte limit",
                false,
            )
            .with_http_status(status)
            .with_provider_request_id(provider_request_id(&headers)));
        }

        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(classify_transport_error)?;
            if body.len().saturating_add(chunk.len()) > self.config.max_response_bytes {
                return Err(ImageGenerationError::new(
                    ImageGenerationErrorCode::ResponseTooLarge,
                    "image-generation provider response exceeds the configured byte limit",
                    false,
                )
                .with_http_status(status)
                .with_provider_request_id(provider_request_id(&headers)));
            }
            body.extend_from_slice(&chunk);
        }

        Ok(ImageHttpResponse {
            status,
            provider_request_id: provider_request_id(&headers),
            retry_after_seconds: retry_after_seconds(&headers),
            body,
        })
    }
}

impl fmt::Debug for ImageHttpClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageHttpClient")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ImageHttpResponse {
    status: u16,
    provider_request_id: Option<String>,
    retry_after_seconds: Option<u64>,
    body: Vec<u8>,
}

impl ImageHttpResponse {
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    #[must_use]
    pub fn provider_request_id(&self) -> Option<&str> {
        self.provider_request_id.as_deref()
    }

    #[must_use]
    pub fn retry_after_seconds(&self) -> Option<u64> {
        self.retry_after_seconds
    }

    pub fn json(&self) -> Result<Value, ImageGenerationError> {
        serde_json::from_slice(&self.body).map_err(|_| {
            ImageGenerationError::new(
                ImageGenerationErrorCode::InvalidResponse,
                "image-generation provider returned invalid JSON",
                false,
            )
            .with_http_status(self.status)
            .with_provider_request_id(self.provider_request_id.clone())
        })
    }

    #[cfg(test)]
    pub(crate) fn from_test_parts(
        status: u16,
        provider_request_id: Option<&str>,
        retry_after_seconds: Option<u64>,
        body: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            status,
            provider_request_id: provider_request_id.and_then(opaque_provider_request_id),
            retry_after_seconds,
            body: body.into(),
        }
    }
}

impl fmt::Debug for ImageHttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageHttpResponse")
            .field("status", &self.status)
            .field("provider_request_id", &self.provider_request_id)
            .field("retry_after_seconds", &self.retry_after_seconds)
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

pub fn validate_endpoint(
    value: &str,
    policy: ImageHttpEndpointPolicy,
) -> Result<Url, ImageGenerationError> {
    let url = Url::parse(value.trim()).map_err(|_| {
        ImageGenerationError::invalid_configuration(
            "image-generation endpoint must be an absolute URL",
        )
    })?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ImageGenerationError::invalid_configuration(
            "image-generation endpoint must not contain user information",
        ));
    }
    if url.fragment().is_some() {
        return Err(ImageGenerationError::invalid_configuration(
            "image-generation endpoint must not contain a fragment",
        ));
    }
    if url.query().is_some() {
        return Err(ImageGenerationError::invalid_configuration(
            "image-generation endpoint must not contain a query string",
        ));
    }
    let Some(host) = url.host_str() else {
        return Err(ImageGenerationError::invalid_configuration(
            "image-generation endpoint must contain a host",
        ));
    };
    match url.scheme() {
        "https" => {}
        "http"
            if policy == ImageHttpEndpointPolicy::AllowLoopbackHttpForTests
                && is_literal_loopback_host(host) => {}
        _ => {
            return Err(ImageGenerationError::invalid_configuration(
                "image-generation endpoint must use HTTPS",
            ));
        }
    }
    Ok(url)
}

fn is_literal_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn bearer_header(credential: &CredentialSecret) -> Result<HeaderValue, ImageGenerationError> {
    credential.with_secret_bytes(|secret| {
        let mut bytes = Vec::with_capacity(b"Bearer ".len() + secret.len());
        bytes.extend_from_slice(b"Bearer ");
        bytes.extend_from_slice(secret);
        let result = HeaderValue::from_bytes(&bytes).map_err(|_| {
            ImageGenerationError::invalid_configuration(
                "image-generation credential cannot be represented as an HTTP header",
            )
        });
        bytes.zeroize();
        result.map(|mut header| {
            header.set_sensitive(true);
            header
        })
    })
}

fn classify_transport_error(error: reqwest::Error) -> ImageGenerationError {
    if error.is_timeout() {
        return ImageGenerationError::new(
            ImageGenerationErrorCode::RequestTimedOut,
            "image-generation provider request timed out; the remote outcome may be unknown",
            false,
        );
    }
    if error.is_connect() {
        return ImageGenerationError::new(
            ImageGenerationErrorCode::TransportFailed,
            "image-generation provider connection could not be established",
            true,
        );
    }
    ImageGenerationError::new(
        ImageGenerationErrorCode::TransportFailed,
        "image-generation provider transport failed; the remote outcome may be unknown",
        false,
    )
}

fn provider_request_id(headers: &HeaderMap) -> Option<String> {
    ["x-request-id", "request-id", "x-trace-id"]
        .into_iter()
        .find_map(|name| headers.get(name))
        .and_then(|value| value.to_str().ok())
        .and_then(opaque_provider_request_id)
}

fn retry_after_seconds(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

fn opaque_provider_request_id(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 1_024 || value.chars().any(char::is_control) {
        return None;
    }
    // Request identifiers are untrusted provider input. Hashing preserves correlation without
    // allowing a provider to smuggle a credential or response fragment into Debug/trace fields.
    let digest = Sha256::digest(value.as_bytes());
    let mut opaque = String::with_capacity("sha256:".len() + 32);
    opaque.push_str("sha256:");
    for byte in &digest[..16] {
        write!(&mut opaque, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Some(opaque)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_policy_requires_https_and_rejects_credential_bearing_urls() {
        assert!(validate_endpoint(
            "https://images.example/v1/images/generations",
            ImageHttpEndpointPolicy::HttpsOnly
        )
        .is_ok());
        for endpoint in [
            "http://images.example/v1/images/generations",
            "https://user:password@images.example/v1/images/generations",
            "https://images.example/v1/images/generations#fragment",
            "https://images.example/v1/images/generations?api_key=secret",
        ] {
            assert!(validate_endpoint(endpoint, ImageHttpEndpointPolicy::HttpsOnly).is_err());
        }
    }

    #[test]
    fn explicit_test_policy_allows_only_loopback_http() {
        for endpoint in [
            "http://localhost:3000/v1/images/generations",
            "http://127.0.0.1:3000/v1/images/generations",
            "http://[::1]:3000/v1/images/generations",
        ] {
            assert!(validate_endpoint(
                endpoint,
                ImageHttpEndpointPolicy::AllowLoopbackHttpForTests
            )
            .is_ok());
        }
        assert!(validate_endpoint(
            "http://images.example/v1/images/generations",
            ImageHttpEndpointPolicy::AllowLoopbackHttpForTests
        )
        .is_err());
    }

    #[test]
    fn authorization_header_is_marked_sensitive() {
        let secret = CredentialSecret::new("never-print-this").unwrap();
        let header = bearer_header(&secret).unwrap();
        let debug = format!("{header:?}");
        assert!(!debug.contains("never-print-this"));
        assert!(header.is_sensitive());
    }

    #[test]
    fn response_debug_does_not_include_body() {
        let response = ImageHttpResponse::from_test_parts(
            401,
            Some("request-1"),
            None,
            br#"{"error":"never-log-provider-body"}"#.to_vec(),
        );
        let debug = format!("{response:?}");
        assert!(!debug.contains("never-log-provider-body"));
        assert!(debug.contains("body_bytes"));
    }

    #[test]
    fn provider_request_ids_are_hashed_before_entering_results_or_debug_output() {
        let raw = "secret-shaped-request-id-token";
        let response = ImageHttpResponse::from_test_parts(200, Some(raw), None, b"{}".to_vec());
        let opaque = response.provider_request_id().unwrap();

        assert!(opaque.starts_with("sha256:"));
        assert!(!opaque.contains(raw));
        assert!(!format!("{response:?}").contains(raw));
    }
}
