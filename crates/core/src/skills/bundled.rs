use super::digest::package_revision;
use super::model::{
    ResolvedSkillPackage, SkillActivationScope, SkillCatalog, SkillDescriptor,
    SkillDescriptorParts, SkillDiscoveryError, SkillId, SkillProvenance, SkillReferenceError,
    SkillRegistrationError, SkillResolveError, SkillResourceIndex, SkillSelection, SkillSourceId,
    SkillSourceKind, SkillTrust, SKILL_PACKAGE_FORMAT_VERSION,
};
use super::package::{PackageManifest, PackageManifestEntry, SkillPackagePath};
use super::parser::parse_skill_document;
use super::resource_runtime::{
    SkillResourceError, SkillResourceReader, SkillResourceReaderRef, SkillResourceSourceError,
};
use super::service::finalize_catalog;
use super::source::SkillSource;
use super::workspace::SKILL_FILE_NAME;
use std::collections::BTreeMap;
use std::sync::Arc;

pub const APPLICATION_BUNDLED_SKILL_SOURCE_ID: &str = "bundled:application";
pub const DOCUMENTS_LOCAL_ID: &str = "documents";
pub const IMAGE_GENERATION_LOCAL_ID: &str = "image-generation";
pub const IMAGE_GENERATION_SKILL_ID: &str = "bundled:application:image-generation";
pub const PDF_LOCAL_ID: &str = "pdf";
pub const PRESENTATIONS_LOCAL_ID: &str = "presentations";
pub const SKILL_CREATOR_LOCAL_ID: &str = "skill-creator";
pub const SKILL_INSTALLER_LOCAL_ID: &str = "skill-installer";
pub const SPREADSHEETS_LOCAL_ID: &str = "spreadsheets";

const DOCUMENTS_PATH: &str = "documents/SKILL.md";
const DOCUMENTS_SOURCE: &str = include_str!("bundled/documents/SKILL.md");
const DOCUMENTS_RESOURCES: &[EmbeddedSkillResource] = &[
    EmbeddedSkillResource {
        path: "office-capability.json",
        bytes: include_bytes!("bundled/documents/office-capability.json"),
    },
    EmbeddedSkillResource {
        path: "references/editing-existing.md",
        bytes: include_bytes!("bundled/documents/references/editing-existing.md"),
    },
    EmbeddedSkillResource {
        path: "references/workflows.md",
        bytes: include_bytes!("bundled/documents/references/workflows.md"),
    },
    EmbeddedSkillResource {
        path: "templates/builder.py",
        bytes: include_bytes!("bundled/documents/templates/builder.py"),
    },
    EmbeddedSkillResource {
        path: "templates/editor.py",
        bytes: include_bytes!("bundled/documents/templates/editor.py"),
    },
];

const IMAGE_GENERATION_PATH: &str = "image-generation/SKILL.md";
const IMAGE_GENERATION_SOURCE: &str = include_str!("bundled/image-generation/SKILL.md");

const PDF_PATH: &str = "pdf/SKILL.md";
const PDF_SOURCE: &str = include_str!("bundled/pdf/SKILL.md");
const PDF_RESOURCES: &[EmbeddedSkillResource] = &[
    EmbeddedSkillResource {
        path: "references/creating-and-editing.md",
        bytes: include_bytes!("bundled/pdf/references/creating-and-editing.md"),
    },
    EmbeddedSkillResource {
        path: "references/forms.md",
        bytes: include_bytes!("bundled/pdf/references/forms.md"),
    },
    EmbeddedSkillResource {
        path: "references/reading.md",
        bytes: include_bytes!("bundled/pdf/references/reading.md"),
    },
];

const SKILL_CREATOR_PATH: &str = "skill-creator/SKILL.md";
const SKILL_CREATOR_SOURCE: &str = include_str!("bundled/skill-creator/SKILL.md");
const SKILL_CREATOR_RESOURCES: &[EmbeddedSkillResource] = &[
    EmbeddedSkillResource {
        path: "LICENSE.txt",
        bytes: include_bytes!("bundled/skill-creator/LICENSE.txt"),
    },
    EmbeddedSkillResource {
        path: "NOTICE.txt",
        bytes: include_bytes!("bundled/skill-creator/NOTICE.txt"),
    },
    EmbeddedSkillResource {
        path: "references/evaluation.md",
        bytes: include_bytes!("bundled/skill-creator/references/evaluation.md"),
    },
    EmbeddedSkillResource {
        path: "references/platform-workflows.md",
        bytes: include_bytes!("bundled/skill-creator/references/platform-workflows.md"),
    },
    EmbeddedSkillResource {
        path: "references/resource-layout.md",
        bytes: include_bytes!("bundled/skill-creator/references/resource-layout.md"),
    },
    EmbeddedSkillResource {
        path: "references/schemas.md",
        bytes: include_bytes!("bundled/skill-creator/references/schemas.md"),
    },
    EmbeddedSkillResource {
        path: "references/writing-guide.md",
        bytes: include_bytes!("bundled/skill-creator/references/writing-guide.md"),
    },
    EmbeddedSkillResource {
        path: "templates/starter-skill/SKILL.md",
        bytes: include_bytes!("bundled/skill-creator/templates/starter-skill/SKILL.md"),
    },
];

const SKILL_INSTALLER_PATH: &str = "skill-installer/SKILL.md";
const SKILL_INSTALLER_SOURCE: &str = include_str!("bundled/skill-installer/SKILL.md");

const PRESENTATIONS_PATH: &str = "presentations/SKILL.md";
const PRESENTATIONS_SOURCE: &str = include_str!("bundled/presentations/SKILL.md");
const PRESENTATIONS_RESOURCES: &[EmbeddedSkillResource] = &[
    EmbeddedSkillResource {
        path: "office-capability.json",
        bytes: include_bytes!("bundled/presentations/office-capability.json"),
    },
    EmbeddedSkillResource {
        path: "references/editing-existing.md",
        bytes: include_bytes!("bundled/presentations/references/editing-existing.md"),
    },
    EmbeddedSkillResource {
        path: "references/workflows.md",
        bytes: include_bytes!("bundled/presentations/references/workflows.md"),
    },
    EmbeddedSkillResource {
        path: "templates/builder.mjs",
        bytes: include_bytes!("bundled/presentations/templates/builder.mjs"),
    },
    EmbeddedSkillResource {
        path: "templates/editor.mjs",
        bytes: include_bytes!("bundled/presentations/templates/editor.mjs"),
    },
];

const SPREADSHEETS_PATH: &str = "spreadsheets/SKILL.md";
const SPREADSHEETS_SOURCE: &str = include_str!("bundled/spreadsheets/SKILL.md");
const SPREADSHEETS_RESOURCES: &[EmbeddedSkillResource] = &[
    EmbeddedSkillResource {
        path: "office-capability.json",
        bytes: include_bytes!("bundled/spreadsheets/office-capability.json"),
    },
    EmbeddedSkillResource {
        path: "references/editing-existing.md",
        bytes: include_bytes!("bundled/spreadsheets/references/editing-existing.md"),
    },
    EmbeddedSkillResource {
        path: "references/workflows.md",
        bytes: include_bytes!("bundled/spreadsheets/references/workflows.md"),
    },
    EmbeddedSkillResource {
        path: "templates/builder.py",
        bytes: include_bytes!("bundled/spreadsheets/templates/builder.py"),
    },
    EmbeddedSkillResource {
        path: "templates/editor.py",
        bytes: include_bytes!("bundled/spreadsheets/templates/editor.py"),
    },
];

struct EmbeddedSkillResource {
    path: &'static str,
    bytes: &'static [u8],
}

struct EmbeddedSkill {
    local_id: &'static str,
    relative_path: &'static str,
    source_text: &'static str,
    resources: &'static [EmbeddedSkillResource],
}

const EMBEDDED_SKILLS: &[EmbeddedSkill] = &[
    EmbeddedSkill {
        local_id: DOCUMENTS_LOCAL_ID,
        relative_path: DOCUMENTS_PATH,
        source_text: DOCUMENTS_SOURCE,
        resources: DOCUMENTS_RESOURCES,
    },
    EmbeddedSkill {
        local_id: IMAGE_GENERATION_LOCAL_ID,
        relative_path: IMAGE_GENERATION_PATH,
        source_text: IMAGE_GENERATION_SOURCE,
        resources: &[],
    },
    EmbeddedSkill {
        local_id: PDF_LOCAL_ID,
        relative_path: PDF_PATH,
        source_text: PDF_SOURCE,
        resources: PDF_RESOURCES,
    },
    EmbeddedSkill {
        local_id: PRESENTATIONS_LOCAL_ID,
        relative_path: PRESENTATIONS_PATH,
        source_text: PRESENTATIONS_SOURCE,
        resources: PRESENTATIONS_RESOURCES,
    },
    EmbeddedSkill {
        local_id: SKILL_CREATOR_LOCAL_ID,
        relative_path: SKILL_CREATOR_PATH,
        source_text: SKILL_CREATOR_SOURCE,
        resources: SKILL_CREATOR_RESOURCES,
    },
    EmbeddedSkill {
        local_id: SKILL_INSTALLER_LOCAL_ID,
        relative_path: SKILL_INSTALLER_PATH,
        source_text: SKILL_INSTALLER_SOURCE,
        resources: &[],
    },
    EmbeddedSkill {
        local_id: SPREADSHEETS_LOCAL_ID,
        relative_path: SPREADSHEETS_PATH,
        source_text: SPREADSHEETS_SOURCE,
        resources: SPREADSHEETS_RESOURCES,
    },
];

type EmbeddedResourceBytes = BTreeMap<String, &'static [u8]>;

#[derive(Debug)]
struct EmbeddedSkillSnapshot {
    package: ResolvedSkillPackage,
    resource_bytes: Arc<EmbeddedResourceBytes>,
}

/// A read-only source whose package bytes are compiled into the application.
///
/// Construction validates every embedded document before the source can enter
/// the registry. Resolution therefore returns an immutable, revision-bound
/// snapshot without depending on an installation path or runtime filesystem.
#[derive(Debug)]
pub(super) struct BundledSkillSource {
    source_id: SkillSourceId,
    packages: BTreeMap<SkillId, EmbeddedSkillSnapshot>,
}

impl BundledSkillSource {
    pub fn new() -> Result<Self, SkillRegistrationError> {
        let source_id = SkillSourceId::parse(APPLICATION_BUNDLED_SKILL_SOURCE_ID)
            .map_err(invalid_bundled_source)?;
        let mut packages = BTreeMap::new();

        for embedded in EMBEDDED_SKILLS {
            let snapshot = load_embedded_skill(&source_id, embedded)?;
            if packages
                .insert(snapshot.package.id().clone(), snapshot)
                .is_some()
            {
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
                .map(|snapshot| snapshot.package.descriptor().clone())
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

        let snapshot =
            self.packages
                .get(selection.skill_id())
                .ok_or_else(|| SkillResolveError::NotFound {
                    skill_id: selection.skill_id().clone(),
                })?;
        let package = &snapshot.package;
        if package.revision() != selection.expected_revision() {
            return Err(SkillResolveError::Stale {
                skill_id: selection.skill_id().clone(),
                expected_revision: selection.expected_revision().clone(),
                actual_revision: package.revision().clone(),
            });
        }

        Ok(package.clone())
    }

    fn open_resource_reader(
        &self,
        package: &ResolvedSkillPackage,
    ) -> Result<Option<SkillResourceReaderRef>, SkillResourceError> {
        if package.id().source_id() != &self.source_id {
            return Err(SkillResourceError::SourceContractViolation {
                source_id: self.source_id.clone(),
                reason: format!(
                    "Skill `{}` does not belong to this bundled source",
                    package.id()
                ),
            });
        }
        let snapshot = self.packages.get(package.id()).ok_or_else(|| {
            SkillResourceError::SnapshotUnavailable {
                skill_id: package.id().clone(),
                revision: package.revision().clone(),
                reason: "the embedded Skill package is not present in this application build"
                    .to_string(),
            }
        })?;
        if snapshot.package != *package {
            return Err(SkillResourceError::SourceContractViolation {
                source_id: self.source_id.clone(),
                reason: format!(
                    "Skill `{}` does not match the immutable embedded package snapshot",
                    package.id()
                ),
            });
        }
        if package.resources().is_empty() {
            return Ok(None);
        }

        Ok(Some(Arc::new(BundledSkillResourceReader {
            resources: snapshot.resource_bytes.clone(),
        })))
    }
}

fn load_embedded_skill(
    source_id: &SkillSourceId,
    embedded: &EmbeddedSkill,
) -> Result<EmbeddedSkillSnapshot, SkillRegistrationError> {
    let expected_path = format!("{}/SKILL.md", embedded.local_id);
    if embedded.relative_path != expected_path {
        return Err(SkillRegistrationError::InvalidSource {
            reason: format!(
                "bundled Skill `{}` must use its canonical SKILL.md path `{expected_path}`",
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

    let mut resource_bytes = BTreeMap::new();
    let (format_version, revision, resources) = if embedded.resources.is_empty() {
        (
            SKILL_PACKAGE_FORMAT_VERSION,
            package_revision(embedded.source_text.as_bytes()),
            SkillResourceIndex::default(),
        )
    } else {
        let mut files = Vec::with_capacity(embedded.resources.len().saturating_add(1));
        files.push(PackageManifestEntry::from_bytes(
            SkillPackagePath::parse(SKILL_FILE_NAME.to_string())
                .map_err(|error| invalid_embedded_package(embedded, error.message))?,
            embedded.source_text.as_bytes(),
        ));
        for resource in embedded.resources {
            let path = SkillPackagePath::parse(resource.path.to_string())
                .map_err(|error| invalid_embedded_package(embedded, error.message))?;
            files.push(PackageManifestEntry::from_bytes(path, resource.bytes));
            if resource_bytes
                .insert(resource.path.to_string(), resource.bytes)
                .is_some()
            {
                return Err(invalid_embedded_package(
                    embedded,
                    format!(
                        "resource path `{}` is declared more than once",
                        resource.path
                    ),
                ));
            }
        }
        let manifest = PackageManifest::new(files)
            .map_err(|error| invalid_embedded_package(embedded, error.message))?;
        (
            manifest.format_version(),
            manifest.revision(),
            SkillResourceIndex::new(manifest.resource_descriptors()),
        )
    };

    let source_text: Arc<str> = Arc::from(embedded.source_text);
    let descriptor = SkillDescriptor::new(SkillDescriptorParts {
        id: skill_id,
        name: document.metadata.name,
        description: document.metadata.description,
        source_kind: SkillSourceKind::Bundled,
        trust: SkillTrust::Application,
        activation_scope: SkillActivationScope::Run,
        revision,
        provenance: SkillProvenance::Bundled {
            source_id: source_id.clone(),
            relative_path: embedded.relative_path.to_string(),
        },
    });

    let package = ResolvedSkillPackage::with_resources(
        descriptor,
        format_version,
        resources,
        source_text,
        document.instructions_range,
    )
    .map_err(|error| SkillRegistrationError::InvalidSource {
        reason: format!(
            "invalid bundled Skill snapshot `{}` at `{}`: {error}",
            embedded.local_id, embedded.relative_path
        ),
    })?;
    Ok(EmbeddedSkillSnapshot {
        package,
        resource_bytes: Arc::new(resource_bytes),
    })
}

fn invalid_embedded_package(
    embedded: &EmbeddedSkill,
    reason: impl Into<String>,
) -> SkillRegistrationError {
    SkillRegistrationError::InvalidSource {
        reason: format!(
            "invalid bundled Skill package `{}` at `{}`: {}",
            embedded.local_id,
            embedded.relative_path,
            reason.into()
        ),
    }
}

#[derive(Debug)]
struct BundledSkillResourceReader {
    resources: Arc<EmbeddedResourceBytes>,
}

impl SkillResourceReader for BundledSkillResourceReader {
    fn read(
        &self,
        expected: &super::model::SkillResourceDescriptor,
    ) -> Result<Vec<u8>, SkillResourceSourceError> {
        self.resources
            .get(expected.path())
            .map(|bytes| bytes.to_vec())
            .ok_or_else(|| {
                SkillResourceSourceError::Unavailable(format!(
                    "embedded resource `{}` is not present in this application build",
                    expected.path()
                ))
            })
    }
}

fn invalid_bundled_source(error: SkillReferenceError) -> SkillRegistrationError {
    SkillRegistrationError::InvalidSource {
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests;
