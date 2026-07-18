use std::fs;
use std::path::Path;

use mycopilot_core::skills::SKILL_PACKAGE_FORMAT_VERSION;
use serde_json::json;
use sha2::{Digest, Sha256};

const PACKAGE_DOMAIN: &[u8] = b"mycopilot.skill.package\0";

/// Materialize the public managed-store format without reaching into the
/// source implementation, matching how a future installer will publish data.
pub(crate) fn write_installed_skill(
    store_root: &Path,
    installation_id: &str,
    source_text: &str,
) -> String {
    let revision = package_revision(source_text.as_bytes());
    let digest = revision
        .strip_prefix("skill-package-sha256-v1:")
        .expect("test package revision must have the v1 prefix");
    let package_directory = store_root.join("packages").join("v1").join(digest);
    fs::create_dir_all(&package_directory).unwrap();
    fs::write(package_directory.join("SKILL.md"), source_text).unwrap();

    let installation_directory = store_root.join("installations");
    fs::create_dir_all(&installation_directory).unwrap();
    let receipt = json!({
        "schemaVersion": 1,
        "installationId": installation_id,
        "package": {
            "formatVersion": SKILL_PACKAGE_FORMAT_VERSION,
            "revision": revision.clone(),
            "entrypoint": "SKILL.md"
        },
        "origin": {
            "provider": "test-fixture",
            "reference": "local"
        },
        "installedAtUnixMs": 1
    });
    fs::write(
        installation_directory.join(format!("{installation_id}.json")),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();

    revision
}

fn package_revision(source_bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(PACKAGE_DOMAIN);
    digest.update(SKILL_PACKAGE_FORMAT_VERSION.to_be_bytes());
    digest.update((source_bytes.len() as u64).to_be_bytes());
    digest.update(source_bytes);
    digest.update(0_u64.to_be_bytes());
    let digest = digest.finalize();
    format!("skill-package-sha256-v1:{digest:x}")
}
