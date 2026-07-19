use super::*;

#[cfg(test)]
pub(crate) fn catalog_response(catalog: &SkillCatalog) -> Result<SkillsListResponse, String> {
    catalog_response_filtered(catalog, |_| true, catalog.catalog_revision().to_string())
}

/// Builds the picker catalog from the durable global enablement snapshot.
///
/// Workspace Skills are request-scoped and intentionally unaffected by the
/// global settings switch. A storage failure is returned to the caller so the
/// picker cannot optimistically expose a Skill whose eligibility is unknown.
pub(crate) fn enabled_catalog_response(
    storage: &StorageService,
    catalog: &SkillCatalog,
) -> Result<SkillsListResponse, String> {
    let managed_ids = catalog
        .skills()
        .iter()
        .filter(|skill| {
            matches!(
                skill.source_kind(),
                SkillSourceKind::Bundled | SkillSourceKind::Installed
            )
        })
        .map(|skill| skill.id().as_str().to_string())
        .collect::<Vec<_>>();
    let enablement = storage.load_skill_enablement(&managed_ids)?;
    let is_enabled = |descriptor: &SkillDescriptor| {
        descriptor.source_kind() == SkillSourceKind::Workspace
            || enablement.get(descriptor.id().as_str()) == Some(&true)
    };
    let revision = enabled_catalog_revision(catalog, &enablement);
    catalog_response_filtered(catalog, is_enabled, revision)
}

pub(super) fn catalog_response_filtered(
    catalog: &SkillCatalog,
    include: impl Fn(&SkillDescriptor) -> bool,
    catalog_revision: String,
) -> Result<SkillsListResponse, String> {
    Ok(SkillsListResponse {
        schema_version: SKILL_CATALOG_SCHEMA_VERSION,
        catalog_revision,
        skills: catalog
            .skills()
            .iter()
            .filter(|descriptor| include(descriptor))
            .map(descriptor_dto)
            .collect::<Result<Vec<_>, _>>()?,
        diagnostics: catalog
            .diagnostics()
            .iter()
            .map(|diagnostic| SkillDiagnosticDto {
                code: diagnostic.code().stable_name().to_string(),
                severity: match diagnostic.severity() {
                    SkillDiagnosticSeverity::Warning => "warning",
                    SkillDiagnosticSeverity::Error => "error",
                    _ => "error",
                }
                .to_string(),
                message: diagnostic.message().to_string(),
                skill_id: None,
                location: Some(diagnostic.path().to_string()),
            })
            .collect(),
        truncated: catalog.truncated(),
    })
}

pub(super) fn enabled_catalog_revision(
    catalog: &SkillCatalog,
    enablement: &std::collections::BTreeMap<String, bool>,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"mycopilot.skill.enabled-catalog\0");
    digest.update(1_u32.to_be_bytes());
    update_digest_bytes(&mut digest, catalog.catalog_revision().as_bytes());
    digest.update((catalog.skills().len() as u64).to_be_bytes());
    for descriptor in catalog.skills() {
        update_digest_bytes(&mut digest, descriptor.id().as_str().as_bytes());
        let enabled = match descriptor.source_kind() {
            SkillSourceKind::Workspace => true,
            SkillSourceKind::Bundled | SkillSourceKind::Installed => enablement
                .get(descriptor.id().as_str())
                .copied()
                .unwrap_or(false),
            _ => false,
        };
        digest.update([u8::from(enabled)]);
    }
    let bytes = digest.finalize();
    let mut revision = String::from("skill-enabled-catalog-sha256-v1:");
    for byte in bytes {
        write!(&mut revision, "{byte:02x}").expect("writing to a String cannot fail");
    }
    revision
}

pub(super) fn update_digest_bytes(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
}
