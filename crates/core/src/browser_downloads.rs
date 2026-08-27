use serde_json::{json, Map, Value};
use std::collections::HashSet;
use uuid::{Uuid, Version};

const MAX_DOWNLOAD_REFERENCES: usize = 16;
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const REFERENCE_FIELDS: [&str; 8] = [
    "schemaVersion",
    "downloadId",
    "displayName",
    "mimeType",
    "sizeBytes",
    "sha256",
    "createdAt",
    "source",
];

/// Reconstructs path-free durable download references from an untrusted Host Tool result.
pub fn safe_browser_download_references(structured: &Value) -> Option<Vec<Value>> {
    let downloads = structured.as_object()?.get("downloads")?.as_array()?;
    if downloads.is_empty() || downloads.len() > MAX_DOWNLOAD_REFERENCES {
        return None;
    }
    let mut ids = HashSet::with_capacity(downloads.len());
    let mut safe = Vec::with_capacity(downloads.len());
    for download in downloads {
        let projected = safe_browser_download_reference(download)?;
        let id = projected.get("downloadId")?.as_str()?;
        if !ids.insert(id.to_string()) {
            return None;
        }
        safe.push(projected);
    }
    Some(safe)
}

fn safe_browser_download_reference(value: &Value) -> Option<Value> {
    let record = value.as_object()?;
    if record.len() != REFERENCE_FIELDS.len()
        || !REFERENCE_FIELDS
            .iter()
            .all(|field| record.contains_key(*field))
        || record.get("schemaVersion")?.as_u64()? != 1
    {
        return None;
    }
    let download_id = record.get("downloadId")?.as_str()?;
    let uuid_text = download_id.strip_prefix("browser-download:")?;
    let uuid = Uuid::parse_str(uuid_text).ok()?;
    if uuid.get_version() != Some(Version::Random) || uuid.hyphenated().to_string() != uuid_text {
        return None;
    }
    let display_name = record.get("displayName")?.as_str()?;
    if !valid_display_name(display_name) {
        return None;
    }
    let mime_type = record.get("mimeType")?.as_str()?;
    if !valid_mime_type(mime_type) {
        return None;
    }
    let size_bytes = safe_u64(record, "sizeBytes")?;
    let created_at = safe_u64(record, "createdAt")?;
    if size_bytes > MAX_DOWNLOAD_BYTES {
        return None;
    }
    let sha256 = record.get("sha256")?.as_str()?;
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    let source = record.get("source")?.as_str()?;
    if source != "agent" {
        return None;
    }
    Some(json!({
        "schemaVersion": 1,
        "downloadId": download_id,
        "displayName": display_name,
        "mimeType": mime_type,
        "sizeBytes": size_bytes,
        "sha256": sha256,
        "createdAt": created_at,
        "source": "agent",
    }))
}

fn safe_u64(record: &Map<String, Value>, field: &str) -> Option<u64> {
    let value = record.get(field)?.as_u64()?;
    (value <= MAX_SAFE_INTEGER).then_some(value)
}

pub(crate) fn valid_display_name(value: &str) -> bool {
    !value.is_empty()
        && value.encode_utf16().count() <= 255
        && value.trim() == value
        && !value.starts_with('.')
        && !value.ends_with('.')
        && !value
            .chars()
            .any(|character| character.is_control() || "<>:\"/\\|?*".contains(character))
}

pub(crate) fn valid_mime_type(value: &str) -> bool {
    if value != value.to_ascii_lowercase() {
        return false;
    }
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

    fn reference() -> Value {
        json!({
            "schemaVersion": 1,
            "downloadId": "browser-download:123e4567-e89b-42d3-a456-426614174000",
            "displayName": "archive.zip",
            "mimeType": "application/zip",
            "sizeBytes": 351,
            "sha256": "a".repeat(64),
            "createdAt": 1_000,
            "source": "agent"
        })
    }

    #[test]
    fn accepts_only_path_free_agent_downloads() {
        let reference_value = reference();
        assert_eq!(
            safe_browser_download_references(&json!({"downloads": [reference_value.clone()]})),
            Some(vec![reference_value.clone()])
        );
        let mut with_path = reference_value;
        with_path["absolutePath"] = json!("/Users/private/Downloads/archive.zip");
        assert!(safe_browser_download_references(&json!({"downloads": [with_path]})).is_none());

        let mut empty = reference();
        empty["sizeBytes"] = json!(0);
        assert_eq!(
            safe_browser_download_references(&json!({"downloads": [empty.clone()]})),
            Some(vec![empty])
        );

        for invalid_name in [" ../archive.zip", ".hidden", "<unsafe>.zip"] {
            let mut unsafe_reference = reference();
            unsafe_reference["displayName"] = json!(invalid_name);
            assert!(
                safe_browser_download_references(&json!({"downloads": [unsafe_reference]}))
                    .is_none()
            );
        }
    }

    #[test]
    fn rejects_manual_or_duplicate_references() {
        let mut manual = reference();
        manual["source"] = json!("manual");
        assert!(safe_browser_download_references(&json!({"downloads": [manual]})).is_none());
        let reference = reference();
        assert!(safe_browser_download_references(
            &json!({"downloads": [reference.clone(), reference]})
        )
        .is_none());
    }
}
