use super::*;
use crate::skills::digest::{
    PACKAGE_REVISION_PREFIX, PACKAGE_REVISION_V2_PREFIX, PACKAGE_REVISION_V3_PREFIX,
};
use crate::skills::{
    SkillPackageOrigin, SkillSourceKind, SkillsService, USER_INSTALLED_SKILL_SOURCE_ID,
};
use std::sync::{Arc, Barrier};
use tempfile::tempdir;

const INSTALLATION_ID: &str = "01234567-89ab-4def-8123-456789abcdef";
const SECOND_INSTALLATION_ID: &str = "11111111-2222-4333-8444-555555555555";

fn installation_id(value: &str) -> SkillInstallationId {
    SkillInstallationId::parse(value).unwrap()
}

fn package(name: &str, marker: &str) -> PreparedSkillPackage {
    PreparedSkillPackage::from_bytes(
        format!(
            "---\nname: {name}\ndescription: Managed fixture {name}.\n---\n# Instructions\n{marker}\n"
        )
        .into_bytes(),
        SkillPackageOrigin::new("local-directory", format!("fixture:{name}")).unwrap(),
    )
    .unwrap()
}

fn package_v2(name: &str, marker: &str, resource: &[u8]) -> PreparedSkillPackage {
    PreparedSkillPackage::from_files(
        vec![
            (
                SKILL_FILE_NAME.to_string(),
                format!(
                    "---\nname: {name}\ndescription: Managed fixture {name}.\n---\n# Instructions\n{marker}\n"
                )
                .into_bytes(),
            ),
            ("references/guide.md".to_string(), resource.to_vec()),
            ("assets/data.bin".to_string(), vec![0, 1, 2, 255]),
        ],
        SkillPackageOrigin::new("local-directory", format!("fixture:{name}")).unwrap(),
    )
    .unwrap()
}

fn package_v3(name: &str, marker: &str) -> PreparedSkillPackage {
    PreparedSkillPackage::from_files(
        vec![
            (
                SKILL_FILE_NAME.to_string(),
                format!(
                    "---\nname: {name}\ndescription: Managed fixture {name}.\n---\n# Instructions\n{marker}\n"
                )
                .into_bytes(),
            ),
            ("README.md".to_string(), b"ROOT_RESOURCE".to_vec()),
            (
                "agents/openai.yaml".to_string(),
                b"interface: chat".to_vec(),
            ),
            ("references/guide.md".to_string(), b"GUIDE".to_vec()),
        ],
        SkillPackageOrigin::new("local-directory", format!("fixture:{name}")).unwrap(),
    )
    .unwrap()
}

fn provenance(authority: &str, refresh: Option<&str>) -> SkillInstallationProvenance {
    use crate::skills::{SkillInstallationAuthority, SkillInstallationRefresh};

    SkillInstallationProvenance::new(
        SkillInstallationAuthority::new("fixture", 1, authority).unwrap(),
        refresh.map(|payload| SkillInstallationRefresh::new("fixture", 1, payload).unwrap()),
    )
}

fn install_request(id: &str, package: PreparedSkillPackage) -> ManagedSkillInstallRequest {
    let mut request =
        ManagedSkillInstallRequest::with_installation_id(installation_id(id), package);
    request.set_created_at_unix_ms(1_784_347_513_399);
    request
}

fn install_request_with_provenance(
    id: &str,
    package: PreparedSkillPackage,
    provenance: SkillInstallationProvenance,
) -> ManagedSkillInstallRequest {
    let mut request = ManagedSkillInstallRequest::with_installation_id_and_provenance(
        installation_id(id),
        package,
        provenance,
    );
    request.set_created_at_unix_ms(1_784_347_513_399);
    request
}

fn installed_catalog(root: &Path) -> crate::skills::SkillCatalog {
    SkillsService::new()
        .with_installed_source(root)
        .unwrap()
        .list()
        .unwrap()
}

fn installed_receipt(root: &Path, id: &SkillInstallationId) -> InstalledSkillReceipt {
    ManagedSkillStore::new(root)
        .unwrap()
        .load_receipt(id)
        .unwrap()
}

fn installation_revision(root: &Path, id: &SkillInstallationId) -> SkillInstallationRevision {
    installed_receipt(root, id).installation_revision
}

fn package_path(root: &Path, revision: &SkillRevision) -> PathBuf {
    root.join(PACKAGES_DIRECTORY)
        .join(PACKAGE_V1_DIRECTORY)
        .join(
            revision
                .as_str()
                .strip_prefix(PACKAGE_REVISION_PREFIX)
                .unwrap(),
        )
}

fn package_staging_entries(root: &Path) -> Vec<PathBuf> {
    let packages = root.join(PACKAGES_DIRECTORY);
    [
        PACKAGE_V1_DIRECTORY,
        PACKAGE_V2_DIRECTORY,
        PACKAGE_V3_DIRECTORY,
    ]
    .into_iter()
    .flat_map(|version| {
        fs::read_dir(packages.join(version))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(is_owned_package_staging_name)
            })
            .map(|entry| entry.path())
            .collect::<Vec<_>>()
    })
    .collect()
}

mod install_update;
mod resilience;
