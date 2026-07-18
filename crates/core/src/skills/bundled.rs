use super::digest::package_revision;
use super::model::{
    ResolvedSkillPackage, SkillActivationScope, SkillCatalog, SkillDescriptor,
    SkillDescriptorParts, SkillDiscoveryError, SkillId, SkillProvenance, SkillReferenceError,
    SkillRegistrationError, SkillResolveError, SkillSelection, SkillSourceId, SkillSourceKind,
    SkillTrust,
};
use super::parser::parse_skill_document;
use super::service::finalize_catalog;
use super::source::SkillSource;
use std::collections::BTreeMap;
use std::sync::Arc;

pub const APPLICATION_BUNDLED_SKILL_SOURCE_ID: &str = "bundled:application";
pub const REPOSITORY_EVIDENCE_AUDITOR_LOCAL_ID: &str = "repository-evidence-auditor";

const REPOSITORY_EVIDENCE_AUDITOR_PATH: &str = "repository-evidence-auditor/SKILL.md";
const REPOSITORY_EVIDENCE_AUDITOR_SOURCE: &str =
    include_str!("bundled/repository-evidence-auditor/SKILL.md");

struct EmbeddedSkill {
    local_id: &'static str,
    relative_path: &'static str,
    source_text: &'static str,
}

const EMBEDDED_SKILLS: &[EmbeddedSkill] = &[EmbeddedSkill {
    local_id: REPOSITORY_EVIDENCE_AUDITOR_LOCAL_ID,
    relative_path: REPOSITORY_EVIDENCE_AUDITOR_PATH,
    source_text: REPOSITORY_EVIDENCE_AUDITOR_SOURCE,
}];

/// A read-only source whose package bytes are compiled into the application.
///
/// Construction validates every embedded document before the source can enter
/// the registry. Resolution therefore returns an immutable, revision-bound
/// snapshot without depending on an installation path or runtime filesystem.
#[derive(Debug)]
pub(super) struct BundledSkillSource {
    source_id: SkillSourceId,
    packages: BTreeMap<SkillId, ResolvedSkillPackage>,
}

impl BundledSkillSource {
    pub fn new() -> Result<Self, SkillRegistrationError> {
        let source_id = SkillSourceId::parse(APPLICATION_BUNDLED_SKILL_SOURCE_ID)
            .map_err(invalid_bundled_source)?;
        let mut packages = BTreeMap::new();

        for embedded in EMBEDDED_SKILLS {
            let package = load_embedded_skill(&source_id, embedded)?;
            if packages.insert(package.id().clone(), package).is_some() {
                return Err(SkillRegistrationError::InvalidSource {
                    reason: format!(
                        "bundled Skill id `{}` is declared more than once",
                        embedded.local_id
                    ),
                });
            }
        }

        Ok(Self {
            source_id,
            packages,
        })
    }

    pub fn source_id(&self) -> &SkillSourceId {
        &self.source_id
    }
}

impl SkillSource for BundledSkillSource {
    fn id(&self) -> &SkillSourceId {
        self.source_id()
    }

    fn kind(&self) -> SkillSourceKind {
        SkillSourceKind::Bundled
    }

    fn trust(&self) -> SkillTrust {
        SkillTrust::Application
    }

    fn activation_scope(&self) -> SkillActivationScope {
        SkillActivationScope::Run
    }

    fn list(&self) -> Result<SkillCatalog, SkillDiscoveryError> {
        Ok(finalize_catalog(
            self.packages
                .values()
                .map(|package| package.descriptor().clone())
                .collect(),
            Vec::new(),
            false,
        ))
    }

    fn resolve(
        &self,
        selection: &SkillSelection,
    ) -> Result<ResolvedSkillPackage, SkillResolveError> {
        if selection.skill_id().source_id() != &self.source_id {
            return Err(SkillResolveError::InvalidReference {
                reason: format!(
                    "Skill `{}` does not belong to source `{}`",
                    selection.skill_id(),
                    self.source_id
                ),
            });
        }

        let package =
            self.packages
                .get(selection.skill_id())
                .ok_or_else(|| SkillResolveError::NotFound {
                    skill_id: selection.skill_id().clone(),
                })?;
        if package.revision() != selection.expected_revision() {
            return Err(SkillResolveError::Stale {
                skill_id: selection.skill_id().clone(),
                expected_revision: selection.expected_revision().clone(),
                actual_revision: package.revision().clone(),
            });
        }

        Ok(package.clone())
    }
}

fn load_embedded_skill(
    source_id: &SkillSourceId,
    embedded: &EmbeddedSkill,
) -> Result<ResolvedSkillPackage, SkillRegistrationError> {
    let expected_path = format!("{}/SKILL.md", embedded.local_id);
    if embedded.relative_path != expected_path {
        return Err(SkillRegistrationError::InvalidSource {
            reason: format!(
                "bundled Skill `{}` must expose only its canonical SKILL.md path `{expected_path}`",
                embedded.local_id
            ),
        });
    }
    let skill_id = SkillId::from_parts(source_id.clone(), embedded.local_id)
        .map_err(invalid_bundled_source)?;
    let document =
        parse_skill_document(embedded.source_text, embedded.local_id).map_err(|error| {
            SkillRegistrationError::InvalidSource {
                reason: format!(
                    "invalid bundled Skill `{}` at `{}`: {error}",
                    embedded.local_id, embedded.relative_path
                ),
            }
        })?;
    if document.metadata.name_was_defaulted {
        return Err(SkillRegistrationError::InvalidSource {
            reason: format!(
                "bundled Skill `{}` at `{}` must declare an explicit name",
                embedded.local_id, embedded.relative_path
            ),
        });
    }

    let source_text: Arc<str> = Arc::from(embedded.source_text);
    let descriptor = SkillDescriptor::new(SkillDescriptorParts {
        id: skill_id,
        name: document.metadata.name,
        description: document.metadata.description,
        source_kind: SkillSourceKind::Bundled,
        trust: SkillTrust::Application,
        activation_scope: SkillActivationScope::Run,
        revision: package_revision(source_text.as_bytes()),
        provenance: SkillProvenance::Bundled {
            source_id: source_id.clone(),
            relative_path: embedded.relative_path.to_string(),
        },
    });

    ResolvedSkillPackage::new(descriptor, source_text, document.instructions_range).map_err(
        |error| SkillRegistrationError::InvalidSource {
            reason: format!(
                "invalid bundled Skill snapshot `{}` at `{}`: {error}",
                embedded.local_id, embedded.relative_path
            ),
        },
    )
}

fn invalid_bundled_source(error: SkillReferenceError) -> SkillRegistrationError {
    SkillRegistrationError::InvalidSource {
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::model::{SkillErrorCode, SkillRevision, SKILL_PACKAGE_FORMAT_VERSION};

    #[test]
    fn embedded_source_has_a_stable_read_only_package_contract() {
        let source = BundledSkillSource::new().unwrap();
        let catalog = source.list().unwrap();

        assert_eq!(source.id().as_str(), APPLICATION_BUNDLED_SKILL_SOURCE_ID);
        assert_eq!(source.kind(), SkillSourceKind::Bundled);
        assert_eq!(source.trust(), SkillTrust::Application);
        assert_eq!(source.activation_scope(), SkillActivationScope::Run);
        assert!(catalog.diagnostics().is_empty());
        assert!(!catalog.truncated());
        assert_eq!(catalog.skills().len(), 1);

        let descriptor = &catalog.skills()[0];
        assert_eq!(
            descriptor.id().as_str(),
            "bundled:application:repository-evidence-auditor"
        );
        assert_eq!(descriptor.name(), REPOSITORY_EVIDENCE_AUDITOR_LOCAL_ID);
        assert_eq!(descriptor.source_kind(), SkillSourceKind::Bundled);
        assert_eq!(descriptor.trust(), SkillTrust::Application);
        assert_eq!(descriptor.activation_scope(), SkillActivationScope::Run);
        assert!(matches!(
            descriptor.provenance(),
            SkillProvenance::Bundled { source_id, relative_path }
                if source_id == source.id()
                    && relative_path == REPOSITORY_EVIDENCE_AUDITOR_PATH
        ));

        let package = source.resolve(&descriptor.selection()).unwrap();
        assert_eq!(package.format_version(), SKILL_PACKAGE_FORMAT_VERSION);
        assert!(package.resources().is_empty());
        assert_eq!(package.source_text(), REPOSITORY_EVIDENCE_AUDITOR_SOURCE);
        assert!(package
            .instructions()
            .contains("grounding every material conclusion in evidence"));
        assert!(!package.instructions().contains("description:"));
        assert_eq!(
            package.revision(),
            &package_revision(REPOSITORY_EVIDENCE_AUDITOR_SOURCE.as_bytes())
        );
    }

    #[test]
    fn embedded_source_rejects_stale_and_unknown_selections() {
        let source = BundledSkillSource::new().unwrap();
        let descriptor = source.list().unwrap().skills()[0].clone();
        let stale = SkillSelection::new(
            descriptor.id().clone(),
            SkillRevision::parse("stale").unwrap(),
        );
        let stale_error = source.resolve(&stale).unwrap_err();
        assert_eq!(stale_error.code(), SkillErrorCode::Stale);
        assert_eq!(stale_error.actual_revision(), Some(descriptor.revision()));

        let unknown_id = SkillId::from_parts(source.id().clone(), "unknown-bundled-skill").unwrap();
        let unknown = SkillSelection::new(unknown_id, descriptor.revision().clone());
        assert_eq!(
            source.resolve(&unknown).unwrap_err().code(),
            SkillErrorCode::NotFound
        );
    }
}
