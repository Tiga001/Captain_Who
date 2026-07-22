use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::error::Error;
use std::fmt;

pub const MAX_IMAGE_GENERATION_PROMPT_BYTES: usize = 32 * 1024;
pub const MAX_IMAGE_GENERATION_INPUT_BYTES: usize = 20 * 1024 * 1024;
pub const IMAGE_GENERATION_OUTPUT_COUNT: u8 = 1;

/// Validated stable identifier for an image-generation adapter implementation.
///
/// This is deliberately an open identifier rather than a closed vendor enum. The current
/// configuration protocol exposes only adapters supported by this application build, while the
/// provider registry and execution pipeline can accept new factories without adding a central
/// vendor match to core execution code.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ImageGenerationAdapterId(Cow<'static, str>);

impl ImageGenerationAdapterId {
    #[allow(non_upper_case_globals)]
    pub const SmartMlSeedream: Self = Self(Cow::Borrowed("smartmlSeedream"));

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

impl fmt::Display for ImageGenerationAdapterId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TryFrom<&str> for ImageGenerationAdapterId {
    type Error = ImageGenerationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > 128
            || value.trim() != value
            || !value
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic())
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(ImageGenerationError::invalid_configuration(
                "image-generation adapter id is invalid",
            ));
        }
        Ok(Self(Cow::Owned(value.to_string())))
    }
}

impl TryFrom<String> for ImageGenerationAdapterId {
    type Error = ImageGenerationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str()).map(|_| Self(Cow::Owned(value)))
    }
}

impl From<ImageGenerationAdapterId> for String {
    fn from(value: ImageGenerationAdapterId) -> Self {
        value.0.into_owned()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageGenerationOperation {
    Status,
    Generate,
    Edit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ImageGenerationSizePreset {
    #[serde(rename = "2K")]
    TwoK,
}

impl ImageGenerationSizePreset {
    #[must_use]
    pub const fn as_provider_value(self) -> &'static str {
        match self {
            Self::TwoK => "2K",
        }
    }
}

impl fmt::Display for ImageGenerationSizePreset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_provider_value())
    }
}

impl TryFrom<&str> for ImageGenerationSizePreset {
    type Error = ImageGenerationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "2K" => Ok(Self::TwoK),
            _ => Err(ImageGenerationError::invalid_configuration(
                "image-generation size preset is unsupported",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenerationDefaults {
    pub size_preset: ImageGenerationSizePreset,
    pub watermark: bool,
}

impl Default for ImageGenerationDefaults {
    fn default() -> Self {
        Self {
            size_preset: ImageGenerationSizePreset::TwoK,
            watermark: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenerationCapabilities {
    pub text_to_image: bool,
    pub image_to_image: bool,
    pub supported_size_presets: Vec<ImageGenerationSizePreset>,
    pub max_input_images: u8,
    pub max_output_images: u8,
    pub output_transport: ImageGenerationOutputTransport,
}

impl ImageGenerationCapabilities {
    #[must_use]
    pub fn smartml_seedream(image_to_image: bool) -> Self {
        Self {
            text_to_image: true,
            image_to_image,
            supported_size_presets: vec![ImageGenerationSizePreset::TwoK],
            max_input_images: u8::from(image_to_image),
            max_output_images: IMAGE_GENERATION_OUTPUT_COUNT,
            output_transport: ImageGenerationOutputTransport::Url,
        }
    }

    pub fn validate(&self) -> Result<(), ImageGenerationError> {
        if !self.text_to_image {
            return Err(ImageGenerationError::invalid_configuration(
                "text-to-image is required by the first image-generation contract",
            ));
        }
        if self.supported_size_presets != [ImageGenerationSizePreset::TwoK] {
            return Err(ImageGenerationError::invalid_configuration(
                "only the 2K size preset is supported by the first image-generation contract",
            ));
        }
        if self.max_output_images != IMAGE_GENERATION_OUTPUT_COUNT {
            return Err(ImageGenerationError::invalid_configuration(
                "the first image-generation contract produces exactly one image",
            ));
        }
        let expected_input_images = u8::from(self.image_to_image);
        if self.max_input_images != expected_input_images {
            return Err(ImageGenerationError::invalid_configuration(
                "image input capacity does not match the image-to-image capability",
            ));
        }
        if self.output_transport != ImageGenerationOutputTransport::Url {
            return Err(ImageGenerationError::invalid_configuration(
                "the first image-generation contract requires URL output transport",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageGenerationOutputTransport {
    Url,
}

/// Credential-free, immutable runtime profile for one configured provider.
///
/// The host resolves a credential separately and supplies a short-lived
/// [`CredentialSecret`](super::credential_store::CredentialSecret) borrow only when executing a
/// prepared request. This profile is therefore safe to persist and include in frozen actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenerationProviderProfile {
    pub id: String,
    pub adapter_id: ImageGenerationAdapterId,
    pub endpoint_url: String,
    pub model_id: String,
    pub revision: u64,
    pub capabilities: ImageGenerationCapabilities,
    pub defaults: ImageGenerationDefaults,
}

impl ImageGenerationProviderProfile {
    pub fn new(
        id: impl Into<String>,
        adapter_id: ImageGenerationAdapterId,
        endpoint_url: impl Into<String>,
        model_id: impl Into<String>,
        revision: u64,
        capabilities: ImageGenerationCapabilities,
        defaults: ImageGenerationDefaults,
    ) -> Result<Self, ImageGenerationError> {
        let profile = Self {
            id: id.into(),
            adapter_id,
            endpoint_url: endpoint_url.into(),
            model_id: model_id.into(),
            revision,
            capabilities,
            defaults,
        };
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<(), ImageGenerationError> {
        if self.id.trim().is_empty() || self.id.len() > 256 || self.id.chars().any(char::is_control)
        {
            return Err(ImageGenerationError::invalid_configuration(
                "image-generation profile id is invalid",
            ));
        }
        if self.endpoint_url.trim().is_empty()
            || self.endpoint_url.len() > 4_096
            || self.endpoint_url.chars().any(char::is_control)
        {
            return Err(ImageGenerationError::invalid_configuration(
                "image-generation endpoint is invalid",
            ));
        }
        if self.model_id.trim().is_empty()
            || self.model_id.len() > 512
            || self.model_id.chars().any(char::is_control)
        {
            return Err(ImageGenerationError::invalid_configuration(
                "image-generation model id is invalid",
            ));
        }
        if self.revision == 0 {
            return Err(ImageGenerationError::invalid_configuration(
                "image-generation profile revision must be positive",
            ));
        }
        self.capabilities.validate()?;
        if !self
            .capabilities
            .supported_size_presets
            .contains(&self.defaults.size_preset)
        {
            return Err(ImageGenerationError::invalid_configuration(
                "default image size is not supported by the provider profile",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageGenerationRequest {
    Status,
    Generate(ImageGenerationGenerateRequest),
    Edit(ImageGenerationEditRequest),
}

impl ImageGenerationRequest {
    pub fn normalize(
        self,
        profile: &ImageGenerationProviderProfile,
    ) -> Result<NormalizedImageGenerationRequest, ImageGenerationError> {
        profile.validate()?;
        match self {
            Self::Status => Ok(NormalizedImageGenerationRequest {
                operation: ImageGenerationOperation::Status,
                prompt: None,
                inputs: Vec::new(),
                size_preset: profile.defaults.size_preset,
                watermark: profile.defaults.watermark,
                output_count: IMAGE_GENERATION_OUTPUT_COUNT,
            }),
            Self::Generate(request) => {
                let prompt = normalize_prompt(request.prompt)?;
                let size_preset = request.size_preset.unwrap_or(profile.defaults.size_preset);
                validate_size_preset(profile, size_preset)?;
                Ok(NormalizedImageGenerationRequest {
                    operation: ImageGenerationOperation::Generate,
                    prompt: Some(prompt),
                    inputs: Vec::new(),
                    size_preset,
                    watermark: profile.defaults.watermark,
                    output_count: IMAGE_GENERATION_OUTPUT_COUNT,
                })
            }
            Self::Edit(request) => {
                if !profile.capabilities.image_to_image {
                    return Err(ImageGenerationError::unsupported_operation(
                        "the configured provider profile does not enable image-to-image",
                    ));
                }
                let prompt = normalize_prompt(request.prompt)?;
                if request.inputs.len() != usize::from(profile.capabilities.max_input_images) {
                    return Err(ImageGenerationError::invalid_request(
                        "image-to-image requires exactly one validated data URL input",
                    ));
                }
                let size_preset = request.size_preset.unwrap_or(profile.defaults.size_preset);
                validate_size_preset(profile, size_preset)?;
                Ok(NormalizedImageGenerationRequest {
                    operation: ImageGenerationOperation::Edit,
                    prompt: Some(prompt),
                    inputs: request.inputs,
                    size_preset,
                    watermark: profile.defaults.watermark,
                    output_count: IMAGE_GENERATION_OUTPUT_COUNT,
                })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationGenerateRequest {
    pub prompt: String,
    pub size_preset: Option<ImageGenerationSizePreset>,
}

impl ImageGenerationGenerateRequest {
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            size_preset: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationEditRequest {
    pub prompt: String,
    pub inputs: Vec<ImageGenerationDataUrlInput>,
    pub size_preset: Option<ImageGenerationSizePreset>,
}

impl ImageGenerationEditRequest {
    #[must_use]
    pub fn new(prompt: impl Into<String>, input: ImageGenerationDataUrlInput) -> Self {
        Self {
            prompt: prompt.into(),
            inputs: vec![input],
            size_preset: None,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ImageGenerationDataUrlInput {
    media_type: ImageGenerationInputMediaType,
    data_url: String,
    decoded_size_bytes: usize,
    sha256: String,
}

impl ImageGenerationDataUrlInput {
    pub fn parse(value: impl Into<String>) -> Result<Self, ImageGenerationError> {
        let value = value.into();
        let (media_type, encoded) = ImageGenerationInputMediaType::split_data_url(&value)?;
        if encoded.is_empty() || encoded.len() > encoded_len_limit(MAX_IMAGE_GENERATION_INPUT_BYTES)
        {
            return Err(ImageGenerationError::invalid_request(
                "image input data URL is empty or exceeds the size limit",
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| {
                ImageGenerationError::invalid_request(
                    "image input data URL contains invalid base64 data",
                )
            })?;
        if bytes.is_empty() || bytes.len() > MAX_IMAGE_GENERATION_INPUT_BYTES {
            return Err(ImageGenerationError::invalid_request(
                "decoded image input is empty or exceeds the size limit",
            ));
        }
        if !media_type.matches_magic_bytes(&bytes) {
            return Err(ImageGenerationError::invalid_request(
                "image input MIME type does not match its encoded bytes",
            ));
        }
        Ok(Self {
            media_type,
            data_url: value,
            decoded_size_bytes: bytes.len(),
            sha256: hex_sha256(&bytes),
        })
    }

    #[must_use]
    pub fn media_type(&self) -> ImageGenerationInputMediaType {
        self.media_type
    }

    #[must_use]
    pub fn data_url(&self) -> &str {
        &self.data_url
    }

    #[must_use]
    pub fn decoded_size_bytes(&self) -> usize {
        self.decoded_size_bytes
    }

    /// Returns a content digest suitable for frozen execution identity and audit metadata.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

impl fmt::Debug for ImageGenerationDataUrlInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageGenerationDataUrlInput")
            .field("media_type", &self.media_type)
            .field("decoded_size_bytes", &self.decoded_size_bytes)
            .field("sha256", &self.sha256)
            .field("data_url", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageGenerationInputMediaType {
    Png,
    Jpeg,
    Webp,
}

impl ImageGenerationInputMediaType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }

    fn split_data_url(value: &str) -> Result<(Self, &str), ImageGenerationError> {
        const TYPES: [(&str, ImageGenerationInputMediaType); 3] = [
            ("data:image/png;base64,", ImageGenerationInputMediaType::Png),
            (
                "data:image/jpeg;base64,",
                ImageGenerationInputMediaType::Jpeg,
            ),
            (
                "data:image/webp;base64,",
                ImageGenerationInputMediaType::Webp,
            ),
        ];
        TYPES
            .into_iter()
            .find_map(|(prefix, media_type)| {
                value
                    .strip_prefix(prefix)
                    .map(|encoded| (media_type, encoded))
            })
            .ok_or_else(|| {
                ImageGenerationError::invalid_request(
                    "image input must be a PNG, JPEG, or WebP base64 data URL",
                )
            })
    }

    fn matches_magic_bytes(self, bytes: &[u8]) -> bool {
        match self {
            Self::Png => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            Self::Jpeg => bytes.starts_with(&[0xff, 0xd8, 0xff]),
            Self::Webp => {
                bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP"
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct NormalizedImageGenerationRequest {
    operation: ImageGenerationOperation,
    prompt: Option<String>,
    inputs: Vec<ImageGenerationDataUrlInput>,
    size_preset: ImageGenerationSizePreset,
    watermark: bool,
    output_count: u8,
}

impl NormalizedImageGenerationRequest {
    #[must_use]
    pub fn operation(&self) -> ImageGenerationOperation {
        self.operation
    }

    #[must_use]
    pub fn prompt(&self) -> Option<&str> {
        self.prompt.as_deref()
    }

    #[must_use]
    pub fn inputs(&self) -> &[ImageGenerationDataUrlInput] {
        &self.inputs
    }

    #[must_use]
    pub fn size_preset(&self) -> ImageGenerationSizePreset {
        self.size_preset
    }

    #[must_use]
    pub fn watermark(&self) -> bool {
        self.watermark
    }

    #[must_use]
    pub fn output_count(&self) -> u8 {
        self.output_count
    }
}

impl fmt::Debug for NormalizedImageGenerationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NormalizedImageGenerationRequest")
            .field("operation", &self.operation)
            .field("prompt_bytes", &self.prompt.as_ref().map(String::len))
            .field("input_count", &self.inputs.len())
            .field("size_preset", &self.size_preset)
            .field("watermark", &self.watermark)
            .field("output_count", &self.output_count)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PreparedImageGenerationRequest {
    provider_profile_id: String,
    adapter_id: ImageGenerationAdapterId,
    profile_revision: u64,
    endpoint_url: String,
    model_id: String,
    normalized: NormalizedImageGenerationRequest,
}

impl PreparedImageGenerationRequest {
    pub(crate) fn new(
        profile: &ImageGenerationProviderProfile,
        normalized: NormalizedImageGenerationRequest,
    ) -> Self {
        Self {
            provider_profile_id: profile.id.clone(),
            adapter_id: profile.adapter_id.clone(),
            profile_revision: profile.revision,
            endpoint_url: profile.endpoint_url.clone(),
            model_id: profile.model_id.clone(),
            normalized,
        }
    }

    #[must_use]
    pub fn provider_profile_id(&self) -> &str {
        &self.provider_profile_id
    }

    #[must_use]
    pub fn adapter_id(&self) -> &ImageGenerationAdapterId {
        &self.adapter_id
    }

    #[must_use]
    pub fn profile_revision(&self) -> u64 {
        self.profile_revision
    }

    #[must_use]
    pub fn endpoint_url(&self) -> &str {
        &self.endpoint_url
    }

    #[must_use]
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    #[must_use]
    pub fn normalized(&self) -> &NormalizedImageGenerationRequest {
        &self.normalized
    }
}

impl fmt::Debug for PreparedImageGenerationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedImageGenerationRequest")
            .field("provider_profile_id", &self.provider_profile_id)
            .field("adapter_id", &self.adapter_id)
            .field("profile_revision", &self.profile_revision)
            .field("endpoint_url", &self.endpoint_url)
            .field("model_id", &self.model_id)
            .field("normalized", &self.normalized)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageGenerationResultStatus {
    Ready,
    Succeeded,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ImageGenerationUrlOutput {
    url: String,
}

impl ImageGenerationUrlOutput {
    pub(crate) fn new(url: String) -> Self {
        Self { url }
    }

    /// Returns the ephemeral provider URL for immediate transfer into the Artifact pipeline.
    /// Callers must not persist it in conversation traces or user-visible diagnostics.
    #[must_use]
    pub fn expose_url_for_download(&self) -> &str {
        &self.url
    }
}

impl fmt::Debug for ImageGenerationUrlOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ImageGenerationUrlOutput([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationResult {
    pub status: ImageGenerationResultStatus,
    pub provider_profile_id: String,
    pub adapter_id: ImageGenerationAdapterId,
    pub profile_revision: u64,
    pub model_id: String,
    pub operation: ImageGenerationOperation,
    pub http_status: Option<u16>,
    pub provider_request_id: Option<String>,
    pub outputs: Vec<ImageGenerationUrlOutput>,
    pub capabilities: Option<ImageGenerationCapabilities>,
}

impl ImageGenerationResult {
    #[must_use]
    pub fn ready(profile: &ImageGenerationProviderProfile) -> Self {
        Self {
            status: ImageGenerationResultStatus::Ready,
            provider_profile_id: profile.id.clone(),
            adapter_id: profile.adapter_id.clone(),
            profile_revision: profile.revision,
            model_id: profile.model_id.clone(),
            operation: ImageGenerationOperation::Status,
            http_status: None,
            provider_request_id: None,
            outputs: Vec::new(),
            capabilities: Some(profile.capabilities.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageGenerationErrorCode {
    InvalidConfiguration,
    InvalidRequest,
    UnsupportedOperation,
    AuthenticationFailed,
    RateLimited,
    ProviderRejected,
    ProviderUnavailable,
    RequestTimedOut,
    ResponseTooLarge,
    InvalidResponse,
    TransportFailed,
    ProfileConflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationError {
    pub code: ImageGenerationErrorCode,
    pub message: String,
    pub http_status: Option<u16>,
    pub provider_code: Option<String>,
    pub provider_request_id: Option<String>,
    pub retry_after_seconds: Option<u64>,
    pub retryable: bool,
}

impl ImageGenerationError {
    #[must_use]
    pub fn new(
        code: ImageGenerationErrorCode,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            http_status: None,
            provider_code: None,
            provider_request_id: None,
            retry_after_seconds: None,
            retryable,
        }
    }

    #[must_use]
    pub fn invalid_configuration(message: impl Into<String>) -> Self {
        Self::new(
            ImageGenerationErrorCode::InvalidConfiguration,
            message,
            false,
        )
    }

    #[must_use]
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ImageGenerationErrorCode::InvalidRequest, message, false)
    }

    #[must_use]
    pub fn unsupported_operation(message: impl Into<String>) -> Self {
        Self::new(
            ImageGenerationErrorCode::UnsupportedOperation,
            message,
            false,
        )
    }

    #[must_use]
    pub fn profile_conflict(message: impl Into<String>) -> Self {
        Self::new(ImageGenerationErrorCode::ProfileConflict, message, false)
    }

    #[must_use]
    pub fn with_http_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }

    #[must_use]
    pub fn with_provider_code(mut self, code: Option<String>) -> Self {
        self.provider_code = code;
        self
    }

    #[must_use]
    pub fn with_provider_request_id(mut self, request_id: Option<String>) -> Self {
        self.provider_request_id = request_id;
        self
    }

    #[must_use]
    pub fn with_retry_after_seconds(mut self, seconds: Option<u64>) -> Self {
        self.retry_after_seconds = seconds;
        self
    }
}

impl fmt::Display for ImageGenerationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ImageGenerationError {}

fn normalize_prompt(prompt: String) -> Result<String, ImageGenerationError> {
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() || prompt.len() > MAX_IMAGE_GENERATION_PROMPT_BYTES {
        return Err(ImageGenerationError::invalid_request(
            "image-generation prompt is empty or exceeds the size limit",
        ));
    }
    Ok(prompt)
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn validate_size_preset(
    profile: &ImageGenerationProviderProfile,
    size_preset: ImageGenerationSizePreset,
) -> Result<(), ImageGenerationError> {
    if profile
        .capabilities
        .supported_size_presets
        .contains(&size_preset)
    {
        Ok(())
    } else {
        Err(ImageGenerationError::unsupported_operation(
            "the requested image size is not supported by the provider profile",
        ))
    }
}

const fn encoded_len_limit(decoded_bytes: usize) -> usize {
    decoded_bytes.div_ceil(3) * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(image_to_image: bool) -> ImageGenerationProviderProfile {
        ImageGenerationProviderProfile::new(
            "default",
            ImageGenerationAdapterId::SmartMlSeedream,
            "https://images.example/v1/images/generations",
            "seedream-model",
            1,
            ImageGenerationCapabilities::smartml_seedream(image_to_image),
            ImageGenerationDefaults::default(),
        )
        .unwrap()
    }

    fn png_data_url() -> String {
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\ncontent")
        )
    }

    #[test]
    fn first_contract_requires_text_to_image_and_exact_defaults() {
        let mut capabilities = ImageGenerationCapabilities::smartml_seedream(false);
        capabilities.text_to_image = false;
        assert_eq!(
            capabilities.validate().unwrap_err().code,
            ImageGenerationErrorCode::InvalidConfiguration
        );

        let defaults = ImageGenerationDefaults::default();
        assert_eq!(defaults.size_preset, ImageGenerationSizePreset::TwoK);
        assert!(defaults.watermark);
    }

    #[test]
    fn adapter_ids_are_open_but_strictly_validated() {
        let adapter = ImageGenerationAdapterId::try_from("acme.images-v2").unwrap();
        assert_eq!(adapter.as_str(), "acme.images-v2");
        let encoded = serde_json::to_string(&adapter).unwrap();
        assert_eq!(encoded, r#""acme.images-v2""#);
        assert_eq!(
            serde_json::from_str::<ImageGenerationAdapterId>(&encoded).unwrap(),
            adapter
        );
        assert!(ImageGenerationAdapterId::try_from("../unsafe").is_err());
        assert!(serde_json::from_str::<ImageGenerationAdapterId>(r#"" bad""#).is_err());
    }

    #[test]
    fn generate_normalization_applies_frozen_profile_defaults() {
        let normalized = ImageGenerationRequest::Generate(ImageGenerationGenerateRequest::new(
            "  a moonlit train  ",
        ))
        .normalize(&profile(false))
        .unwrap();

        assert_eq!(normalized.operation(), ImageGenerationOperation::Generate);
        assert_eq!(normalized.prompt(), Some("a moonlit train"));
        assert_eq!(normalized.size_preset(), ImageGenerationSizePreset::TwoK);
        assert!(normalized.watermark());
        assert_eq!(normalized.output_count(), 1);
        assert!(normalized.inputs().is_empty());
    }

    #[test]
    fn edit_requires_capability_and_a_valid_data_url() {
        let input = ImageGenerationDataUrlInput::parse(png_data_url()).unwrap();
        let request = ImageGenerationRequest::Edit(ImageGenerationEditRequest::new(
            "change the background",
            input.clone(),
        ));
        assert_eq!(
            request.clone().normalize(&profile(false)).unwrap_err().code,
            ImageGenerationErrorCode::UnsupportedOperation
        );

        let normalized = request.normalize(&profile(true)).unwrap();
        assert_eq!(normalized.operation(), ImageGenerationOperation::Edit);
        assert_eq!(normalized.inputs(), [input]);
    }

    #[test]
    fn data_url_debug_never_contains_payload() {
        let value = png_data_url();
        let input = ImageGenerationDataUrlInput::parse(value.clone()).unwrap();
        let debug = format!("{input:?}");
        assert!(!debug.contains(&value));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn data_url_rejects_mime_spoofing() {
        let value = format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\ncontent")
        );
        assert_eq!(
            ImageGenerationDataUrlInput::parse(value).unwrap_err().code,
            ImageGenerationErrorCode::InvalidRequest
        );
    }

    #[test]
    fn prepared_request_debug_omits_prompt_and_input_bytes() {
        let request = ImageGenerationRequest::Edit(ImageGenerationEditRequest::new(
            "private prompt",
            ImageGenerationDataUrlInput::parse(png_data_url()).unwrap(),
        ))
        .normalize(&profile(true))
        .unwrap();
        let prepared = PreparedImageGenerationRequest::new(&profile(true), request);
        let debug = format!("{prepared:?}");
        assert!(!debug.contains("private prompt"));
        assert!(!debug.contains("data:image"));
    }
}
