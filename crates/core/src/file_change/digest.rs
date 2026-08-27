use serde::Serialize;
use sha2::{Digest, Sha256};

const CONTENT_DOMAIN: &[u8] = b"mycopilot.file-change.content\0";
const DIFF_DOMAIN: &[u8] = b"mycopilot.file-change.diff\0";
const PROPOSAL_DOMAIN: &[u8] = b"mycopilot.file-change.proposal\0";
const DIGEST_PREFIX: &str = "file-change-sha256-v1:";

pub fn content_digest(content: &[u8]) -> String {
    digest_bytes(CONTENT_DOMAIN, content)
}

pub fn diff_digest(diff: &str) -> String {
    digest_bytes(DIFF_DOMAIN, diff.as_bytes())
}

pub fn proposal_digest<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let material = serde_json::to_vec(value)?;
    Ok(digest_bytes(PROPOSAL_DOMAIN, &material))
}

fn digest_bytes(domain: &[u8], value: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
    format!("{DIGEST_PREFIX}{:x}", hasher.finalize())
}

pub(crate) fn valid_digest(value: &str) -> bool {
    value.strip_prefix(DIGEST_PREFIX).is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
