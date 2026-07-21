use super::model::{
    SkillActivationRevision, SkillDescriptor, SkillDiagnostic, SkillProvenance, SkillRevision,
    SKILL_PACKAGE_FORMAT_VERSION,
};
use super::package::PackageManifestEntry;
use sha2::{Digest, Sha256};
use std::fmt::Write;

const PACKAGE_DOMAIN: &[u8] = b"mycopilot.skill.package\0";
const PACKAGE_FILE_DOMAIN: &[u8] = b"mycopilot.skill.file\0";
const ACTIVATION_DOMAIN: &[u8] = b"mycopilot.skill.activation\0";
const CATALOG_DOMAIN: &[u8] = b"mycopilot.skill.catalog\0";
pub(super) const PACKAGE_REVISION_PREFIX: &str = "skill-package-sha256-v1:";
pub(super) const PACKAGE_REVISION_V2_PREFIX: &str = "skill-package-sha256-v2:";
pub(super) const PACKAGE_REVISION_V3_PREFIX: &str = "skill-package-sha256-v3:";
pub(super) const PACKAGE_FILE_DIGEST_PREFIX: &str = "skill-file-sha256-v1:";

pub(super) fn package_revision(source_bytes: &[u8]) -> SkillRevision {
    // This collision-resistant content token detects changes; it is not an
    // authenticity signature and does not elevate the package's trust.
    let mut digest = Sha256::new();
    digest.update(PACKAGE_DOMAIN);
    digest.update(SKILL_PACKAGE_FORMAT_VERSION.to_be_bytes());
    update_bytes(&mut digest, source_bytes);
    // Package format v1 reserves an explicit resource index but intentionally
    // loads no sibling resources.
    digest.update(0_u64.to_be_bytes());
    SkillRevision::trusted(format_digest(PACKAGE_REVISION_PREFIX, digest.finalize()))
}

pub(super) fn package_file_digest(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(PACKAGE_FILE_DOMAIN);
    digest.update(1_u32.to_be_bytes());
    update_bytes(&mut digest, bytes);
    format_digest(PACKAGE_FILE_DIGEST_PREFIX, digest.finalize())
}

pub(super) fn package_revision_v2(entries: &[PackageManifestEntry]) -> SkillRevision {
    package_tree_revision(2, PACKAGE_REVISION_V2_PREFIX, entries)
}

pub(super) fn package_revision_v3(entries: &[PackageManifestEntry]) -> SkillRevision {
    package_tree_revision(3, PACKAGE_REVISION_V3_PREFIX, entries)
}

fn package_tree_revision(
    format_version: u32,
    prefix: &str,
    entries: &[PackageManifestEntry],
) -> SkillRevision {
    let mut digest = Sha256::new();
    digest.update(PACKAGE_DOMAIN);
    digest.update(format_version.to_be_bytes());
    digest.update((entries.len() as u64).to_be_bytes());
    for entry in entries {
        update_bytes(&mut digest, entry.path().as_bytes());
        update_bytes(&mut digest, entry.kind_name().as_bytes());
        digest.update(entry.byte_length().to_be_bytes());
        update_bytes(&mut digest, entry.digest().as_bytes());
    }
    SkillRevision::trusted(format_digest(prefix, digest.finalize()))
}

pub(super) fn activation_revision<'a>(
    descriptors: impl IntoIterator<Item = &'a SkillDescriptor>,
) -> SkillActivationRevision {
    activation_revision_for_identities(
        descriptors
            .into_iter()
            .map(|descriptor| (descriptor.id().as_str(), descriptor.revision().as_str())),
    )
}

/// Computes the canonical ordered activation-set revision without requiring resolved package
/// bodies. Runtime progressive activation and eager host activation must share this exact digest
/// so the public revision keeps one stable meaning across both entry paths.
pub(crate) fn activation_revision_for_identities<'a>(
    identities: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> SkillActivationRevision {
    let identities = identities.into_iter().collect::<Vec<_>>();
    let mut digest = Sha256::new();
    digest.update(ACTIVATION_DOMAIN);
    digest.update(1_u32.to_be_bytes());
    digest.update((identities.len() as u64).to_be_bytes());
    for (id, revision) in identities {
        update_bytes(&mut digest, id.as_bytes());
        update_bytes(&mut digest, revision.as_bytes());
    }
    SkillActivationRevision::trusted(format_digest(
        "skill-activation-sha256-v1:",
        digest.finalize(),
    ))
}

pub(super) fn catalog_revision(
    skills: &[SkillDescriptor],
    diagnostics: &[SkillDiagnostic],
    truncated: bool,
) -> String {
    let mut digest = Sha256::new();
    digest.update(CATALOG_DOMAIN);
    digest.update(1_u32.to_be_bytes());
    digest.update([u8::from(truncated)]);
    digest.update((skills.len() as u64).to_be_bytes());
    for skill in skills {
        update_bytes(&mut digest, skill.id().as_str().as_bytes());
        update_bytes(&mut digest, skill.name().as_bytes());
        update_bytes(&mut digest, skill.description().as_bytes());
        update_bytes(&mut digest, skill.revision().as_str().as_bytes());
        update_bytes(&mut digest, skill.source_kind().stable_name().as_bytes());
        update_bytes(&mut digest, skill.trust().stable_name().as_bytes());
        update_bytes(
            &mut digest,
            skill.activation_scope().stable_name().as_bytes(),
        );
        update_provenance(&mut digest, skill.provenance());
    }
    digest.update((diagnostics.len() as u64).to_be_bytes());
    for diagnostic in diagnostics {
        update_bytes(&mut digest, diagnostic.code().stable_name().as_bytes());
        update_bytes(&mut digest, diagnostic.severity().stable_name().as_bytes());
        update_bytes(&mut digest, diagnostic.path().as_bytes());
        update_bytes(&mut digest, diagnostic.message().as_bytes());
    }
    format_digest("skill-catalog-sha256-v1:", digest.finalize())
}

fn update_provenance(digest: &mut Sha256, provenance: &SkillProvenance) {
    match provenance {
        SkillProvenance::Workspace {
            workspace_id,
            relative_path,
        } => {
            update_bytes(digest, b"workspace");
            update_bytes(digest, workspace_id.as_bytes());
            update_bytes(digest, relative_path.as_bytes());
        }
        SkillProvenance::Bundled {
            source_id,
            relative_path,
        } => {
            update_bytes(digest, b"bundled");
            update_bytes(digest, source_id.as_str().as_bytes());
            update_bytes(digest, relative_path.as_bytes());
        }
        SkillProvenance::Installed {
            source_id,
            installation_id,
            relative_path,
        } => {
            update_bytes(digest, b"installed");
            update_bytes(digest, source_id.as_str().as_bytes());
            update_bytes(digest, installation_id.as_str().as_bytes());
            update_bytes(digest, relative_path.as_bytes());
        }
        SkillProvenance::Other {
            source_id,
            display_location,
        } => {
            update_bytes(digest, b"other");
            update_bytes(digest, source_id.as_str().as_bytes());
            match display_location {
                Some(location) => {
                    digest.update([1]);
                    update_bytes(digest, location.as_bytes());
                }
                None => digest.update([0]),
            }
        }
    }
}

fn update_bytes(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
}

fn format_digest(prefix: &str, digest: impl AsRef<[u8]>) -> String {
    let digest = digest.as_ref();
    let mut revision = String::with_capacity(prefix.len() + digest.len() * 2);
    revision.push_str(prefix);
    for byte in digest {
        write!(&mut revision, "{byte:02x}").expect("writing to a String cannot fail");
    }
    revision
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::model::{
        SkillActivationScope, SkillDescriptorParts, SkillId, SkillSourceId, SkillSourceKind,
        SkillTrust,
    };
    use crate::skills::package::{PackageManifest, PackageManifestEntry, SkillPackagePath};

    #[test]
    fn package_revision_is_versioned_deterministic_and_byte_exact() {
        let revision = package_revision(b"skill\n");
        assert!(revision.as_str().starts_with("skill-package-sha256-v1:"));
        assert_eq!(
            revision.as_str().len(),
            "skill-package-sha256-v1:".len() + 64
        );
        assert_eq!(revision, package_revision(b"skill\n"));
        assert_ne!(revision, package_revision(b"skill\r\n"));
    }

    #[test]
    fn package_v2_digest_has_a_cross_platform_golden_vector() {
        let manifest = PackageManifest::new(vec![
            PackageManifestEntry::from_bytes(
                SkillPackagePath::parse("references/a.md").unwrap(),
                b"A",
            ),
            PackageManifestEntry::from_bytes(
                SkillPackagePath::parse("SKILL.md").unwrap(),
                b"skill",
            ),
        ])
        .unwrap();
        assert_eq!(
            package_file_digest(b"skill"),
            "skill-file-sha256-v1:1ff3116893cf39163cd0876408ca9697449c513fdabdfc2902180997a0b075f0"
        );
        assert_eq!(
            manifest.revision().as_str(),
            "skill-package-sha256-v2:c786b51c62a834cc561c3fa158288444b270e3974d1b06796207bcae1e8c1900"
        );
    }

    #[test]
    fn package_v3_digest_is_domain_separated_from_v2() {
        let manifest = PackageManifest::new(vec![
            PackageManifestEntry::from_bytes(
                SkillPackagePath::parse("README.md").unwrap(),
                b"readme",
            ),
            PackageManifestEntry::from_bytes(
                SkillPackagePath::parse("SKILL.md").unwrap(),
                b"skill",
            ),
        ])
        .unwrap();

        assert_eq!(manifest.format_version(), 3);
        assert!(manifest
            .revision()
            .as_str()
            .starts_with(PACKAGE_REVISION_V3_PREFIX));
        assert_ne!(manifest.revision(), package_revision_v2(manifest.files()));
    }

    #[test]
    fn catalog_revision_domain_separates_bundled_and_other_provenance() {
        let source_id = SkillSourceId::parse("bundled:application").unwrap();
        let skill_id = SkillId::from_parts(source_id.clone(), "auditor").unwrap();
        let revision = package_revision(b"skill\n");
        let descriptor = |provenance| {
            SkillDescriptor::new(SkillDescriptorParts {
                id: skill_id.clone(),
                name: "auditor".to_string(),
                description: "Audit a repository.".to_string(),
                source_kind: SkillSourceKind::Bundled,
                trust: SkillTrust::Application,
                activation_scope: SkillActivationScope::Run,
                revision: revision.clone(),
                provenance,
            })
        };
        let bundled = descriptor(SkillProvenance::Bundled {
            source_id: source_id.clone(),
            relative_path: "auditor/SKILL.md".to_string(),
        });
        let other = descriptor(SkillProvenance::Other {
            source_id,
            display_location: Some("auditor/SKILL.md".to_string()),
        });

        assert_ne!(
            catalog_revision(&[bundled], &[], false),
            catalog_revision(&[other], &[], false)
        );
    }

    #[test]
    fn activation_descriptor_and_identity_apis_share_one_revision_contract() {
        let source_id = SkillSourceId::parse("bundled:application").unwrap();
        let descriptor = SkillDescriptor::new(SkillDescriptorParts {
            id: SkillId::from_parts(source_id.clone(), "documents").unwrap(),
            name: "Documents".to_string(),
            description: "Create and edit documents.".to_string(),
            source_kind: SkillSourceKind::Bundled,
            trust: SkillTrust::Application,
            activation_scope: SkillActivationScope::Run,
            revision: package_revision(b"documents skill"),
            provenance: SkillProvenance::Bundled {
                source_id,
                relative_path: "documents/SKILL.md".to_string(),
            },
        });

        assert_eq!(
            activation_revision([&descriptor]),
            activation_revision_for_identities([(
                descriptor.id().as_str(),
                descriptor.revision().as_str(),
            )])
        );
    }
}
