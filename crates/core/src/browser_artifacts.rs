use serde_json::{json, Map, Value};
use std::collections::HashSet;
use uuid::{Uuid, Version};

const MAX_ARTIFACT_REFERENCES: usize = 16;
const MAX_ARTIFACT_BYTES: u64 = 128 * 1024 * 1024;
const MAX_ARTIFACT_LIFETIME_MS: u64 = 24 * 60 * 60 * 1_000;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const REFERENCE_FIELDS: [&str; 11] = [
    "schemaVersion",
    "artifactId",
    "kind",
    "displayName",
    "mimeType",
    "sizeBytes",
    "createdAt",
    "expiresAt",
    "lifecycle",
    "owner",
    "preview",
];

/// Extracts the exact Renderer/model-safe Browser Artifact DTO from an MCP structured result.
///
/// Absence and malformed data both return `None`: callers fail closed by omitting all references,
/// never by retaining the source object. The reconstructed Values contain only the frozen allowlist
/// and therefore cannot carry a managed path, output directory, body, or binary payload.
pub fn safe_browser_artifact_references(structured: &Value) -> Option<Vec<Value>> {
    let artifacts = structured.as_object()?.get("artifacts")?.as_array()?;
    if artifacts.is_empty() || artifacts.len() > MAX_ARTIFACT_REFERENCES {
        return None;
    }
    let mut ids = HashSet::with_capacity(artifacts.len());
    let mut safe = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        let projected = safe_browser_artifact_reference(artifact)?;
        let artifact_id = projected.get("artifactId")?.as_str()?;
        if !ids.insert(artifact_id.to_string()) {
            return None;
        }
        safe.push(projected);
    }
    Some(safe)
}

const IMAGE_ARTIFACT_READ_PATH_PREFIX: &str = "image-artifact://sha256/";

/// Accepts only the canonical `image-artifact://sha256/<64 hex>` URI used by `read_image.path`.
pub fn safe_image_artifact_read_path(value: &Value) -> Option<String> {
    let path = value.as_str()?;
    let digest = path.strip_prefix(IMAGE_ARTIFACT_READ_PATH_PREFIX)?;
    if digest.len() == 64
        && digest
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        Some(path.to_string())
    } else {
        None
    }
}

fn safe_browser_artifact_reference(value: &Value) -> Option<Value> {
    let record = value.as_object()?;
    if record.len() != REFERENCE_FIELDS.len()
        || !REFERENCE_FIELDS
            .iter()
            .all(|field| record.contains_key(*field))
        || record.get("schemaVersion")?.as_u64()? != 1
    {
        return None;
    }
    let artifact_id = record.get("artifactId")?.as_str()?;
    let uuid_text = artifact_id.strip_prefix("browser-artifact:")?;
    let uuid = Uuid::parse_str(uuid_text).ok()?;
    if uuid.is_nil()
        || uuid.get_version() != Some(Version::Random)
        || uuid.hyphenated().to_string() != uuid_text
    {
        return None;
    }
    let kind = allowed(
        record.get("kind")?.as_str()?,
        &[
            "image", "text", "json", "pdf", "trace", "video", "download", "snapshot", "console",
            "network",
        ],
    )?;
    let display_name = record.get("displayName")?.as_str()?;
    if display_name.is_empty()
        || display_name.encode_utf16().count() > 128
        || display_name
            .chars()
            .any(|character| character.is_ascii_control())
        || matches!(display_name, "." | "..")
        || display_name.contains('/')
        || display_name.contains('\\')
    {
        return None;
    }
    let mime_type = record.get("mimeType")?.as_str()?.to_ascii_lowercase();
    if !valid_mime_type(&mime_type) {
        return None;
    }
    let size_bytes = safe_u64(record, "sizeBytes")?;
    let created_at = safe_u64(record, "createdAt")?;
    let expires_at = safe_u64(record, "expiresAt")?;
    if size_bytes > MAX_ARTIFACT_BYTES
        || expires_at <= created_at
        || expires_at - created_at > MAX_ARTIFACT_LIFETIME_MS
    {
        return None;
    }
    if record.get("lifecycle")?.as_str()? != "run"
        || record.get("owner")?.as_str()? != "browser_automation"
    {
        return None;
    }
    let preview = allowed(record.get("preview")?.as_str()?, &["image", "text", "none"])?;
    Some(json!({
        "schemaVersion": 1,
        "artifactId": artifact_id,
        "kind": kind,
        "displayName": display_name,
        "mimeType": mime_type,
        "sizeBytes": size_bytes,
        "createdAt": created_at,
        "expiresAt": expires_at,
        "lifecycle": "run",
        "owner": "browser_automation",
        "preview": preview,
    }))
}

fn safe_u64(record: &Map<String, Value>, field: &str) -> Option<u64> {
    let value = record.get(field)?.as_u64()?;
    (value <= MAX_SAFE_INTEGER).then_some(value)
}

fn allowed<'a>(value: &'a str, values: &[&str]) -> Option<&'a str> {
    values.contains(&value).then_some(value)
}

fn valid_mime_type(value: &str) -> bool {
    let Some((type_name, subtype)) = value.split_once('/') else {
        return false;
    };
    valid_mime_component(type_name, 64) && valid_mime_component(subtype, 128)
}

fn valid_mime_component(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (index > 0
                    && matches!(
                        byte,
                        b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                    ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact() -> Value {
        json!({
            "schemaVersion": 1,
            "artifactId": "browser-artifact:123e4567-e89b-42d3-a456-426614174000",
            "kind": "image",
            "displayName": "page.png",
            "mimeType": "image/png",
            "sizeBytes": 42,
            "createdAt": 1_000,
            "expiresAt": 2_000,
            "lifecycle": "run",
            "owner": "browser_automation",
            "preview": "image"
        })
    }

    #[test]
    fn accepts_only_canonical_image_artifact_read_paths() {
        let path = format!("image-artifact://sha256/{}", "a".repeat(64));
        assert_eq!(safe_image_artifact_read_path(&json!(path)), Some(path));
        assert!(
            safe_image_artifact_read_path(&json!("image-artifact://sha256/not-a-digest")).is_none()
        );
        assert!(safe_image_artifact_read_path(&json!("/tmp/private.png")).is_none());
        assert!(safe_image_artifact_read_path(&json!(format!(
            "image-artifact://sha256/{}",
            "A".repeat(64)
        )))
        .is_none());
        assert!(safe_image_artifact_read_path(&json!("browser-artifact:123")).is_none());
    }

    #[test]
    fn accepts_only_the_frozen_path_free_reference() {
        let artifact = artifact();
        assert_eq!(
            safe_browser_artifact_references(&json!({"artifacts": [artifact.clone()]})),
            Some(vec![artifact.clone()])
        );
        let mut with_path = artifact.clone();
        with_path["managedPath"] = json!("/tmp/private.png");
        assert!(safe_browser_artifact_references(&json!({"artifacts": [with_path]})).is_none());
        let mut traversal = artifact;
        traversal["displayName"] = json!("../../private.png");
        assert!(safe_browser_artifact_references(&json!({"artifacts": [traversal]})).is_none());
    }

    #[test]
    fn rejects_duplicate_or_unbounded_references() {
        let artifact = artifact();
        assert!(safe_browser_artifact_references(
            &json!({"artifacts": [artifact.clone(), artifact]})
        )
        .is_none());
        assert!(safe_browser_artifact_references(&json!({"artifacts": []})).is_none());
    }
}
