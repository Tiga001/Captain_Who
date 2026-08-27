//! Canonical classification for every model-visible resource locator.
//!
//! Classification is deliberately pure. It never expands aliases, touches the filesystem, or
//! grants authority. Consumers must pass the classified locator to their own trusted resolver.

use std::path::Path;
use uuid::Uuid;

const BROWSER_DOWNLOAD_PREFIX: &str = "browser-download:";
const BROWSER_ARTIFACT_PREFIX: &str = "browser-artifact:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceLocator {
    Attachment(String),
    GeneratedArtifact(String),
    SkillResource(String),
    BrowserDownload(String),
    OpaqueBrowserArtifact(String),
    SystemAlias(String),
    Filesystem(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLocatorError {
    message: String,
}

impl ResourceLocatorError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ResourceLocatorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ResourceLocatorError {}

impl ResourceLocator {
    pub fn parse(value: &str) -> Result<Self, ResourceLocatorError> {
        let value = value.trim();
        if value.is_empty()
            || value.contains('\0')
            || value.contains('\n')
            || value.contains('\r')
            || value.chars().count() > 32 * 1024
        {
            return Err(ResourceLocatorError::new(
                "资源位置必须是非空的单行字符串。",
            ));
        }

        if value.starts_with("@attachments/") {
            return Ok(Self::Attachment(value.to_string()));
        }
        if value.starts_with("image-artifact://sha256/") || value.starts_with("artifact://sha256/")
        {
            return Ok(Self::GeneratedArtifact(value.to_string()));
        }
        if value.starts_with("skill://") {
            return Ok(Self::SkillResource(value.to_string()));
        }
        if value.starts_with(BROWSER_DOWNLOAD_PREFIX) {
            validate_browser_download_id(value)?;
            return Ok(Self::BrowserDownload(value.to_string()));
        }
        if value.starts_with(BROWSER_ARTIFACT_PREFIX) {
            return Ok(Self::OpaqueBrowserArtifact(value.to_string()));
        }
        if is_system_alias(value) {
            return Ok(Self::SystemAlias(value.to_string()));
        }
        if has_unrecognized_uri_scheme(value) {
            return Err(ResourceLocatorError::new(
                "资源位置使用了不受支持的 URI scheme。",
            ));
        }
        Ok(Self::Filesystem(value.to_string()))
    }

    pub fn logical_value(&self) -> &str {
        match self {
            Self::Attachment(value)
            | Self::GeneratedArtifact(value)
            | Self::SkillResource(value)
            | Self::BrowserDownload(value)
            | Self::OpaqueBrowserArtifact(value)
            | Self::SystemAlias(value)
            | Self::Filesystem(value) => value,
        }
    }

    pub fn is_virtual(&self) -> bool {
        matches!(
            self,
            Self::Attachment(_)
                | Self::GeneratedArtifact(_)
                | Self::SkillResource(_)
                | Self::BrowserDownload(_)
                | Self::OpaqueBrowserArtifact(_)
        )
    }
}

fn validate_browser_download_id(value: &str) -> Result<(), ResourceLocatorError> {
    let raw = value
        .strip_prefix(BROWSER_DOWNLOAD_PREFIX)
        .ok_or_else(|| ResourceLocatorError::new("浏览器下载引用无效。"))?;
    let parsed = Uuid::parse_str(raw)
        .ok()
        .filter(|uuid| uuid.get_version_num() == 4)
        .ok_or_else(|| ResourceLocatorError::new("浏览器下载引用无效。"))?;
    if format!("{BROWSER_DOWNLOAD_PREFIX}{parsed}") != value {
        return Err(ResourceLocatorError::new("浏览器下载引用无效。"));
    }
    Ok(())
}

fn is_system_alias(value: &str) -> bool {
    ["~", "@home", "@desktop", "@documents", "@downloads"]
        .iter()
        .any(|alias| {
            value == *alias
                || value
                    .strip_prefix(alias)
                    .is_some_and(|rest| rest.starts_with('/') || rest.starts_with('\\'))
        })
}

fn has_unrecognized_uri_scheme(value: &str) -> bool {
    if !value.contains("://") {
        return false;
    }
    // A Windows drive path is a filesystem path, not a URI.
    if cfg!(windows)
        && value.len() >= 3
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value.as_bytes()[1] == b':'
        && matches!(value.as_bytes()[2], b'/' | b'\\')
    {
        return false;
    }
    !Path::new(value).is_absolute()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_supported_logical_resources() {
        assert!(matches!(
            ResourceLocator::parse("@attachments/a/file.txt").unwrap(),
            ResourceLocator::Attachment(_)
        ));
        assert!(matches!(
            ResourceLocator::parse(&format!("artifact://sha256/{}", "a".repeat(64))).unwrap(),
            ResourceLocator::GeneratedArtifact(_)
        ));
        assert!(matches!(
            ResourceLocator::parse("skill://package/a/revision/file.txt").unwrap(),
            ResourceLocator::SkillResource(_)
        ));
        assert!(matches!(
            ResourceLocator::parse("browser-download:123e4567-e89b-42d3-a456-426614174000")
                .unwrap(),
            ResourceLocator::BrowserDownload(_)
        ));
    }

    #[test]
    fn keeps_ephemeral_browser_artifacts_opaque() {
        assert!(matches!(
            ResourceLocator::parse("browser-artifact:123e4567-e89b-42d3-a456-426614174000")
                .unwrap(),
            ResourceLocator::OpaqueBrowserArtifact(_)
        ));
    }

    #[test]
    fn rejects_unknown_uri_schemes_and_noncanonical_download_ids() {
        assert!(ResourceLocator::parse("unknown://resource").is_err());
        assert!(ResourceLocator::parse("browser-download:not-an-id").is_err());
        assert!(
            ResourceLocator::parse("browser-download:123E4567-E89B-42D3-A456-426614174000")
                .is_err()
        );
    }

    #[test]
    fn classifies_system_aliases_before_filesystem_paths() {
        assert!(matches!(
            ResourceLocator::parse("@downloads/archive.zip").unwrap(),
            ResourceLocator::SystemAlias(_)
        ));
        assert!(matches!(
            ResourceLocator::parse("notes/archive.zip").unwrap(),
            ResourceLocator::Filesystem(_)
        ));
    }
}
