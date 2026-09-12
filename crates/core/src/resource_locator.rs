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
        if crate::workspace::parse_workspace_path(value)
            .map_err(ResourceLocatorError::new)?
            .is_some()
        {
            return Ok(Self::Filesystem(value.to_string()));
        }
        if has_unrecognized_scheme_prefix(value) || has_unrecognized_at_namespace(value) {
            return Err(ResourceLocatorError::new(
                "资源位置使用了不受支持的虚拟资源前缀。",
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

fn has_unrecognized_scheme_prefix(value: &str) -> bool {
    // A Windows drive path is a filesystem path, not a URI.
    if cfg!(windows)
        && value.len() >= 3
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value.as_bytes()[1] == b':'
        && matches!(value.as_bytes()[2], b'/' | b'\\')
    {
        return false;
    }
    if Path::new(value).is_absolute() {
        return false;
    }
    let Some(separator) = value.find(':') else {
        return false;
    };
    let scheme = &value[..separator];
    let mut bytes = scheme.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-'))
}

fn has_unrecognized_at_namespace(value: &str) -> bool {
    value.starts_with('@')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

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
            ResourceLocator::parse(&format!("image-artifact://sha256/{}", "b".repeat(64))).unwrap(),
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
    fn every_declared_virtual_prefix_routes_through_the_logical_resource_boundary() {
        for value in [
            "@attachments/a/file.txt".to_string(),
            format!("artifact://sha256/{}", "a".repeat(64)),
            format!("image-artifact://sha256/{}", "b".repeat(64)),
            "skill://package/a/revision/file.txt".to_string(),
            "browser-download:123e4567-e89b-42d3-a456-426614174000".to_string(),
            "browser-artifact:123e4567-e89b-42d3-a456-426614174000".to_string(),
        ] {
            assert!(
                ResourceLocator::parse(&value).unwrap().is_virtual(),
                "{value}"
            );
        }
        for value in ["@downloads/archive.zip", "notes/archive.zip"] {
            assert!(
                !ResourceLocator::parse(value).unwrap().is_virtual(),
                "{value}"
            );
        }
    }

    #[test]
    fn rejects_unknown_virtual_prefixes_and_noncanonical_download_ids() {
        assert!(ResourceLocator::parse("unknown://resource").is_err());
        assert!(ResourceLocator::parse("future-resource:opaque-id").is_err());
        assert!(ResourceLocator::parse("browser-file:opaque-id").is_err());
        assert!(ResourceLocator::parse("@future/resource").is_err());
        assert!(ResourceLocator::parse("browser-download:not-an-id").is_err());
        assert!(
            ResourceLocator::parse("browser-download:123E4567-E89B-42D3-A456-426614174000")
                .is_err()
        );
    }

    #[test]
    fn explicit_filesystem_syntax_disambiguates_prefix_like_file_names() {
        assert!(matches!(
            ResourceLocator::parse("./future-resource:literal.txt").unwrap(),
            ResourceLocator::Filesystem(_)
        ));
        assert!(matches!(
            ResourceLocator::parse("./@future/literal.txt").unwrap(),
            ResourceLocator::Filesystem(_)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_paths_are_not_treated_as_virtual_schemes() {
        assert!(matches!(
            ResourceLocator::parse(r"C:\\files\\notes.txt").unwrap(),
            ResourceLocator::Filesystem(_)
        ));
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

    #[test]
    fn production_locator_consumers_remain_on_the_single_classifier_boundary() {
        struct CurrentConsumer {
            file: &'static str,
            expected_calls: usize,
            reason: &'static str,
        }

        let allowed = [
            CurrentConsumer {
                file: "tools/run_command.rs", expected_calls: 2,
                reason: "classifies structured command paths without rewriting shell text",
            },
            CurrentConsumer {
                file: "office/execution/filesystem.rs", expected_calls: 1,
                reason: "preserves Office locator routing before frozen workspace resolution",
            },
            CurrentConsumer {
                file: "command/artifact_observer.rs", expected_calls: 1,
                reason: "separates explicit virtual output addresses from cwd-relative files",
            },
            CurrentConsumer {
                file: "workspace.rs",
                expected_calls: 2,
                reason: "resolves filesystem addressing and escapes literal paths through the canonical classifier",
            },
            CurrentConsumer {
                file: "file_input.rs",
                expected_calls: 2,
                reason: "builds the typed private file-input authority reference",
            },
            CurrentConsumer {
                file: "tools/context.rs",
                expected_calls: 3,
                reason: "routes context-search logical resources without granting authority",
            },
        ];
        let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut rust_sources = Vec::new();
        collect_rust_sources(&source_root, &mut rust_sources);
        let mut actual = BTreeMap::new();
        for path in rust_sources {
            if path.ends_with("resource_locator.rs") {
                continue;
            }
            let source = fs::read_to_string(&path).unwrap();
            let count = source.matches("ResourceLocator::parse(").count();
            if count > 0 {
                let relative = path
                    .strip_prefix(&source_root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                actual.insert(relative, count);
            }
        }
        let expected = allowed
            .iter()
            .map(|consumer| {
                assert!(!consumer.reason.is_empty());
                (consumer.file.to_string(), consumer.expected_calls)
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(actual, expected);
    }

    fn collect_rust_sources(directory: &Path, output: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect_rust_sources(&path, output);
            } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
                output.push(path);
            }
        }
    }
}
