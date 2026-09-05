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
mod tests {
    use super::*;
    use crate::skills::model::{
        SkillErrorCode, SkillResourceKind, SkillRevision, SKILL_PACKAGE_FORMAT_VERSION_V2,
        SKILL_PACKAGE_FORMAT_VERSION_V3,
    };
    use crate::skills::{
        SkillMaterializationDestination, SkillMaterializationRequest, SkillMaterializationStatus,
        SkillResourceListOptions, SkillResourceMaterializer, SkillResourcePath,
        SkillResourceTextReadOptions, SkillTemplateTreeMaterializationRequest, SkillsService,
    };
    use crate::tools::ToolRegistry;
    use serde_json::Value;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn markdown_json_examples(document_name: &str, markdown: &str) -> Vec<(usize, Value)> {
        let mut examples = Vec::new();
        let mut lines = markdown.lines().enumerate();

        while let Some((line_index, line)) = lines.next() {
            if line.trim() != "```json" {
                continue;
            }

            let mut source = String::new();
            let mut closed = false;
            for (_, line) in lines.by_ref() {
                if line.trim() == "```" {
                    closed = true;
                    break;
                }
                source.push_str(line);
                source.push('\n');
            }
            assert!(
                closed,
                "{document_name}:{} contains an unterminated JSON code block",
                line_index + 1
            );
            let value = serde_json::from_str(&source).unwrap_or_else(|error| {
                panic!(
                    "{document_name}:{} contains invalid JSON: {error}",
                    line_index + 1
                )
            });
            examples.push((line_index + 1, value));
        }

        examples
    }

    fn is_bidirectional_text_control(character: char) -> bool {
        matches!(
            character,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'
                | '\u{202b}'
                | '\u{202c}'
                | '\u{202d}'
                | '\u{202e}'
                | '\u{2066}'
                | '\u{2067}'
                | '\u{2068}'
                | '\u{2069}'
        )
    }

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
        assert_eq!(catalog.skills().len(), 7);
        assert_eq!(
            catalog
                .skills()
                .iter()
                .map(|skill| skill.id().as_str())
                .collect::<Vec<_>>(),
            vec![
                "bundled:application:documents",
                "bundled:application:image-generation",
                "bundled:application:pdf",
                "bundled:application:presentations",
                "bundled:application:skill-creator",
                "bundled:application:skill-installer",
                "bundled:application:spreadsheets",
            ]
        );

        let descriptor = catalog
            .skills()
            .iter()
            .find(|skill| skill.id().local_id() == DOCUMENTS_LOCAL_ID)
            .unwrap();
        assert_eq!(descriptor.id().as_str(), "bundled:application:documents");
        assert_eq!(descriptor.name(), DOCUMENTS_LOCAL_ID);
        assert_eq!(descriptor.source_kind(), SkillSourceKind::Bundled);
        assert_eq!(descriptor.trust(), SkillTrust::Application);
        assert_eq!(descriptor.activation_scope(), SkillActivationScope::Run);
        assert!(matches!(
            descriptor.provenance(),
            SkillProvenance::Bundled { source_id, relative_path }
                if source_id == source.id()
                    && relative_path == DOCUMENTS_PATH
        ));

        let package = source.resolve(&descriptor.selection()).unwrap();
        assert_eq!(package.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V3);
        assert!(!package.resources().is_empty());
        assert_eq!(package.source_text(), DOCUMENTS_SOURCE);
        assert!(package.instructions().contains("Route by intent:"));
        assert!(package.instructions().contains("**Read or verify:**"));
        assert!(package.instructions().contains("**Create a new `.docx`:**"));
        assert!(package
            .instructions()
            .contains("**Edit an existing `.docx`:**"));
        assert!(!package.instructions().contains("description:"));
        assert_eq!(package.revision(), descriptor.revision());
    }

    #[test]
    fn current_authoring_skills_resolve_and_activate_without_retired_writer_guidance() {
        let source = BundledSkillSource::new().unwrap();
        let catalog = source.list().unwrap();
        let service = SkillsService::new().with_bundled_source().unwrap();

        let registry = ToolRegistry::defaults_with_search(None);
        assert!(registry.definition_for("apply_patch").is_some());
        assert!(registry.definition_for("write_file").is_none());
        assert_eq!(
            registry
                .definitions()
                .iter()
                .filter(|definition| definition.name == "apply_patch")
                .count(),
            1
        );

        for local_id in [
            SKILL_CREATOR_LOCAL_ID,
            SKILL_INSTALLER_LOCAL_ID,
            DOCUMENTS_LOCAL_ID,
            SPREADSHEETS_LOCAL_ID,
            PRESENTATIONS_LOCAL_ID,
        ] {
            let descriptor = catalog
                .skills()
                .iter()
                .find(|skill| skill.id().local_id() == local_id)
                .unwrap();
            let package = source.resolve(&descriptor.selection()).unwrap();
            assert!(
                !package.source_text().contains("write_file"),
                "{local_id} still instructs the model to use the retired writer"
            );

            let activated = service.activate(&[descriptor.selection()]).unwrap();
            assert_eq!(activated.skills().len(), 1);
            assert_eq!(activated.skills()[0].id(), descriptor.id());
            assert_eq!(activated.skills()[0].source_text(), package.source_text());
            assert!(!activated.skills()[0].instructions().contains("write_file"));

            let reader = source.open_resource_reader(&package).unwrap();
            for resource in package.resources().entries() {
                let bytes = reader
                    .as_ref()
                    .expect("resource-bearing package must expose a reader")
                    .read(resource)
                    .unwrap();
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    assert!(
                        !text.contains("write_file"),
                        "{local_id}/{} still instructs the model to use the retired writer",
                        resource.path()
                    );
                }
            }

            if matches!(
                local_id,
                DOCUMENTS_LOCAL_ID | SPREADSHEETS_LOCAL_ID | PRESENTATIONS_LOCAL_ID
            ) {
                let reader = reader.expect("Office package must expose resources");
                let capability = package
                    .resources()
                    .get("office-capability.json")
                    .expect("Office package must expose its capability manifest");
                let capability: Value =
                    serde_json::from_slice(&reader.read(capability).unwrap()).unwrap();
                assert_eq!(
                    capability["modes"]["script"]["editTools"],
                    serde_json::json!(["apply_patch"])
                );
                assert!(
                    capability["modes"]["script"]["routes"]
                        .as_object()
                        .is_some_and(|routes| !routes.is_empty()),
                    "{local_id} lost its managed script route"
                );
                assert!(package
                    .resources()
                    .entries()
                    .iter()
                    .any(|resource| resource.path().starts_with("templates/builder.")));
            }
        }
    }

    #[test]
    fn image_generation_is_an_instruction_only_bundled_skill() {
        let source = BundledSkillSource::new().unwrap();
        let descriptor = source
            .list()
            .unwrap()
            .skills()
            .iter()
            .find(|skill| skill.id().local_id() == IMAGE_GENERATION_LOCAL_ID)
            .unwrap()
            .clone();
        let package = source.resolve(&descriptor.selection()).unwrap();

        assert_eq!(
            descriptor.id().as_str(),
            "bundled:application:image-generation"
        );
        assert_eq!(descriptor.name(), IMAGE_GENERATION_LOCAL_ID);
        assert_eq!(descriptor.source_kind(), SkillSourceKind::Bundled);
        assert_eq!(descriptor.trust(), SkillTrust::Application);
        assert!(package.resources().is_empty());
        assert_eq!(package.source_text(), IMAGE_GENERATION_SOURCE);
        assert!(source.open_resource_reader(&package).unwrap().is_none());
        assert!(!package.instructions().contains("description:"));
        assert!(package.instructions().contains("`image_generation`"));
        assert!(package.instructions().contains("`attachments_list`"));
        assert!(package.instructions().contains("exact `readPath`"));
        assert!(package
            .instructions()
            .contains("request.operation=\"generate\""));
        assert!(package
            .instructions()
            .contains("request.operation=\"edit\""));
        assert!(package.instructions().contains("request.inputPath"));
        assert!(package.instructions().contains("Artifact is verified"));
        assert!(package.instructions().contains("## Product watermark"));
        assert!(package
            .instructions()
            .contains("cropping, covering, repainting"));
        assert!(package.instructions().contains("save, and generate again"));
        assert!(!descriptor.description().contains("watermark"));
        assert!(package
            .instructions()
            .contains("do not retry it automatically"));
        assert!(matches!(
            descriptor.provenance(),
            SkillProvenance::Bundled { source_id, relative_path }
                if source_id == source.id() && relative_path == IMAGE_GENERATION_PATH
        ));
    }

    #[test]
    fn pdf_skill_is_a_trusted_package_with_progressive_references() {
        let source = BundledSkillSource::new().unwrap();
        let descriptor = source
            .list()
            .unwrap()
            .skills()
            .iter()
            .find(|skill| skill.id().local_id() == PDF_LOCAL_ID)
            .unwrap()
            .clone();
        let package = source.resolve(&descriptor.selection()).unwrap();

        assert_eq!(descriptor.id().as_str(), "bundled:application:pdf");
        assert_eq!(descriptor.name(), PDF_LOCAL_ID);
        assert_eq!(descriptor.source_kind(), SkillSourceKind::Bundled);
        assert_eq!(descriptor.trust(), SkillTrust::Application);
        assert_eq!(descriptor.activation_scope(), SkillActivationScope::Run);
        assert_eq!(
            descriptor.description(),
            "Read, search, inspect, create, edit, render, and verify PDF files, including scanned documents, complex layouts, tables, figures, and fillable forms. Use whenever the task involves a .pdf file or PDF output."
        );
        assert!(matches!(
            descriptor.provenance(),
            SkillProvenance::Bundled { source_id, relative_path }
                if source_id == source.id() && relative_path == PDF_PATH
        ));

        assert_eq!(package.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V2);
        assert_eq!(package.source_text(), PDF_SOURCE);
        assert_eq!(package.resources().len(), 3);
        assert_eq!(
            package
                .resources()
                .entries()
                .iter()
                .map(|resource| resource.path())
                .collect::<Vec<_>>(),
            vec![
                "references/creating-and-editing.md",
                "references/forms.md",
                "references/reading.md",
            ]
        );
        assert!(package
            .resources()
            .entries()
            .iter()
            .all(|resource| resource.kind() == SkillResourceKind::Reference));

        let instructions = package.instructions();
        for required in [
            "`run_command`",
            "`read_image`",
            "`MYCOPILOT_INPUT_ROOT`",
            "`office_document.render`",
            "workspace-relative PDF `readPath`",
            "without `run_command.inputs`",
            "external file, generated Artifact",
            "top-level `outputs/` directory for files that command creates",
            "complete executable set is exactly",
            "`pdfinfo`, `pdftotext`, `pdftoppm`, `python`, `python3`, and `rg`",
            "`head`, `tail`, `grep`, `sed`, and `awk` are unavailable",
            "pdftotext -layout",
            "--max-count 20",
            "rg --max-count 80 '^'",
            "pdftotext -f FIRST -l LAST",
            "quoted Python heredoc",
            "Do not print an entire large PDF",
            "`historyOpen`",
            "`conversation_history`",
            "references/reading.md",
            "references/creating-and-editing.md",
            "references/forms.md",
        ] {
            assert!(instructions.contains(required), "missing `{required}`");
        }
        for forbidden in [
            "@scratch",
            "brew install",
            "apt-get",
            "pip install",
            "one direct managed invocation",
            "Pipelines, `&&`",
            "single-line",
        ] {
            assert!(
                !package.source_text().contains(forbidden),
                "model-facing PDF Skill leaked forbidden instruction `{forbidden}`"
            );
        }

        let reader = source.open_resource_reader(&package).unwrap().unwrap();
        for resource in package.resources().entries() {
            assert_eq!(resource.kind(), SkillResourceKind::Reference);
            assert_eq!(
                reader.read(resource).unwrap().len() as u64,
                resource.byte_length()
            );
        }
        let reading = package
            .resources()
            .entries()
            .iter()
            .find(|resource| resource.path() == "references/reading.md")
            .unwrap();
        let reading = String::from_utf8(reader.read(reading).unwrap()).unwrap();
        assert!(reading
            .contains("exactly `pdfinfo`, `pdftotext`, `pdftoppm`, `python`, `python3`, and `rg`"));
        assert!(reading.contains("| rg -n -i -C 4 --max-count 20"));
        assert!(reading.contains("| rg --max-count 80 '^'"));
        assert!(reading.contains("pdftotext -f 42 -l 46 -layout"));
        assert!(reading.contains("`historyOpen` with `conversation_history`"));
        assert!(reading.contains("`head`, `tail`, and byte-offset slicing are not recovery paths"));
        assert!(reading.contains("reports extracted-text line numbers, not PDF page numbers"));
        assert!(reading.contains("python - \"$MYCOPILOT_INPUT_ROOT/manual.pdf\" <<'PY'"));
        assert!(reading.contains("pdfinfo \"<exact outputs[].readPath>\""));
        assert!(reading
            .contains("pdftoppm -f 1 -l 12 -png \"<exact outputs[].readPath>\" outputs/qa-page"));
        assert!(reading.contains("Top-level `outputs/` is only the managed command's output"));
        assert!(reading.contains("Do not dump the full text"));
        assert!(!reading.contains("Do not use `grep` or `rg`"));
        assert!(!reading.contains("python -c"));
        let creating = package
            .resources()
            .entries()
            .iter()
            .find(|resource| resource.path() == "references/creating-and-editing.md")
            .unwrap();
        let creating = String::from_utf8(reader.read(creating).unwrap()).unwrap();
        assert!(creating.contains("python - <<'PY'"));
        assert!(creating.contains("Keep stdout to a short status"));
        assert!(!creating.contains("python -c"));
    }

    #[test]
    fn skill_installer_is_an_instruction_only_trusted_bundled_skill() {
        let source = BundledSkillSource::new().unwrap();
        let descriptor = source
            .list()
            .unwrap()
            .skills()
            .iter()
            .find(|skill| skill.id().local_id() == SKILL_INSTALLER_LOCAL_ID)
            .unwrap()
            .clone();
        let package = source.resolve(&descriptor.selection()).unwrap();

        assert_eq!(
            descriptor.id().as_str(),
            "bundled:application:skill-installer"
        );
        assert_eq!(descriptor.trust(), SkillTrust::Application);
        assert_eq!(
            descriptor.description(),
            "Inspect and install unchanged third-party Skills from GitHub links or authorized local paths. Use when the user asks to add, install, or import a Skill without changing its behavior or files. Use skill-creator first when changes are required."
        );
        assert!(package.resources().is_empty());
        assert_eq!(package.source_text(), SKILL_INSTALLER_SOURCE);
        assert!(source.open_resource_reader(&package).unwrap().is_none());
        assert!(package.instructions().contains("`skills_prepare_install`"));
        assert!(package.instructions().contains("`skills_commit_install`"));
        let explain = package
            .instructions()
            .find("tell the user the Skill name")
            .unwrap();
        let commit = package
            .instructions()
            .find("call `skills_commit_install`")
            .unwrap();
        assert!(explain < commit, "public explanation must precede commit");
        assert!(package
            .instructions()
            .contains("only the exact returned `installRef`"));
        assert!(package
            .instructions()
            .contains("in an intermediate progress message"));
        assert!(package
            .instructions()
            .contains("immediately call `skills_commit_install` in the same run"));
        assert!(package
            .instructions()
            .contains("Do not ask for another textual confirmation"));
        assert!(package
            .instructions()
            .contains("valid only in the run that returned it"));
        assert!(package
            .instructions()
            .contains("distinguish an approved and completed installation"));
        assert!(package
            .instructions()
            .contains("discoverable on the next run"));
        assert!(package.instructions().contains("untrusted data"));
        assert!(package.instructions().contains("Never install by calling"));
        assert!(!package.instructions().contains("description:"));
    }

    #[test]
    fn skill_creator_is_a_resource_only_native_workflow_package() {
        let source = BundledSkillSource::new().unwrap();
        let descriptor = source
            .list()
            .unwrap()
            .skills()
            .iter()
            .find(|skill| skill.id().local_id() == SKILL_CREATOR_LOCAL_ID)
            .unwrap()
            .clone();
        let package = source.resolve(&descriptor.selection()).unwrap();

        assert_eq!(
            descriptor.id().as_str(),
            "bundled:application:skill-creator"
        );
        assert_eq!(descriptor.trust(), SkillTrust::Application);
        assert_eq!(descriptor.activation_scope(), SkillActivationScope::Run);
        assert_eq!(
            descriptor.description(),
            "Create, modify, fix, test, or review Skill packages. Use when the user wants a new Skill or wants to change the behavior or files of a Workspace, Installed, or user-authorized external Skill. Make changes in an editable Workspace version. If the request is only to install an unchanged Skill, use skill-installer instead."
        );
        assert!(!descriptor.description().contains("MyCopilot"));
        assert!(matches!(
            descriptor.provenance(),
            SkillProvenance::Bundled { source_id, relative_path }
                if source_id == source.id() && relative_path == SKILL_CREATOR_PATH
        ));
        assert_eq!(package.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V3);
        assert!(package
            .revision()
            .as_str()
            .starts_with("skill-package-sha256-v3:"));
        assert_eq!(package.source_text(), SKILL_CREATOR_SOURCE);
        assert_eq!(
            package
                .resources()
                .entries()
                .iter()
                .map(|resource| (resource.path(), resource.kind()))
                .collect::<Vec<_>>(),
            vec![
                ("LICENSE.txt", SkillResourceKind::Other),
                ("NOTICE.txt", SkillResourceKind::Other),
                ("references/evaluation.md", SkillResourceKind::Reference),
                (
                    "references/platform-workflows.md",
                    SkillResourceKind::Reference,
                ),
                (
                    "references/resource-layout.md",
                    SkillResourceKind::Reference,
                ),
                ("references/schemas.md", SkillResourceKind::Reference),
                ("references/writing-guide.md", SkillResourceKind::Reference,),
                ("templates/starter-skill/SKILL.md", SkillResourceKind::Other,),
            ]
        );
        assert!(!package
            .resources()
            .entries()
            .iter()
            .any(|resource| resource.kind() == SkillResourceKind::Script));

        let normalized_instructions = package
            .instructions()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for required in [
            "fork_turns: \"none\"",
            "`source=workspace`",
            "Every new revision needs a new reviewer",
            "copy-first boundary is a hard gate",
            "one fresh direct child Agent",
            "each blind case",
            "revision mismatch",
            "cannot PASS",
            "only the original natural-language user request",
            "mark the case `UNVERIFIED`",
            "Do not combine explicit activation and blind triggering",
            "byte-for-byte snapshot",
            "Do not reconstruct the source with read/write tools",
            "use `run_command.inputs`",
            "wait for it in the same Run",
            "does not require an HTML reviewer",
            "does not replace an existing installation in place",
            "remove only those files",
            "Never edit an Installed Skill's application-managed package in place",
            "copy only the manifest-listed Skill files into Workspace",
        ] {
            assert!(
                normalized_instructions.contains(required),
                "skill-creator instructions are missing `{required}`"
            );
        }
        for forbidden in ["claude -p", "generate_review.py", "aggregate_benchmark.py"] {
            assert!(
                !package.source_text().contains(forbidden),
                "skill-creator leaked unsupported workflow `{forbidden}`"
            );
        }
        assert!(!normalized_instructions
            .contains("the platform cannot guarantee a byte-for-byte export of every entry"));

        let reader = source.open_resource_reader(&package).unwrap().unwrap();
        let platform_workflows = package
            .resources()
            .entries()
            .iter()
            .find(|resource| resource.path() == "references/platform-workflows.md")
            .unwrap();
        let platform_workflows =
            String::from_utf8(reader.read(platform_workflows).unwrap()).unwrap();
        let normalized_platform_workflows = platform_workflows
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for required in [
            "Authorized external source",
            "Copy an Installed package",
            "real filesystem byte copy",
            "cp -R --",
            "below 16,000 characters",
            "`cmp -s --",
            "ordinary `shasum` is not equivalent",
            "test ! -L '.agents/skills/<skill-directory>' && mv -n",
            "every segment with `&&`",
            "same filesystem",
            "skill-file-sha256-v1:",
            "This is the only inline verification code authorized",
            "not a Host-level atomic no-replace primitive",
            "On Windows",
            "Re-read the manifest, live receipt",
            "retired-installations/<installation-id>.json",
            "Do not copy the manifest itself",
            "MYCOPILOT_APP_DATA_ROOT",
            "Never place the suffix inside the double quotes",
            "redirect provider diagnostics",
            "create and begin/create omit `observationId`",
            "renews the same ID after the Tool Result is received",
            "`apply_patch` Direct or Staged actions",
            "never use a command, redirection, or script as an alternate writer",
        ] {
            assert!(
                normalized_platform_workflows.contains(required),
                "skill-creator platform workflow is missing `{required}`"
            );
        }
        assert!(!normalized_platform_workflows.contains("printenv MYCOPILOT_APP_DATA_ROOT"));
        let evaluation = package
            .resources()
            .entries()
            .iter()
            .find(|resource| resource.path() == "references/evaluation.md")
            .unwrap();
        let evaluation = String::from_utf8(reader.read(evaluation).unwrap()).unwrap();
        let normalized_evaluation = evaluation.split_whitespace().collect::<Vec<_>>().join(" ");
        for required in [
            "one new direct child with `fork_turns: \"none\"` for each review case",
            "Never put reports, fixtures, generated artifacts, or `test-outputs`",
            "establishes the exact review package revision",
            "cancelled, interrupted, missing its final report",
            "Use a separate fresh child for every blind case",
            "must be exactly the original natural-language user request",
            "Host-provided or otherwise trusted child execution traces",
            "mark the blind case `UNVERIFIED`",
            "independent child Runs",
            "invalidates all of its verdicts",
        ] {
            assert!(
                normalized_evaluation.contains(required),
                "skill-creator evaluation workflow is missing `{required}`"
            );
        }
        for resource in package.resources().entries() {
            assert_eq!(
                reader.read(resource).unwrap().len() as u64,
                resource.byte_length()
            );
        }

        let service = SkillsService::new().with_bundled_source().unwrap();
        let activated = service.activate(&[descriptor.selection()]).unwrap();
        let resources = service.resource_session(&activated).unwrap();
        let package_uri = resources.package_uris().into_iter().next().unwrap();
        let workspace = tempdir().unwrap();
        fs::create_dir(workspace.path().join("skill-creator-tmp-01")).unwrap();
        let request = SkillTemplateTreeMaterializationRequest::new(
            package_uri,
            SkillResourcePath::parse("templates/starter-skill").unwrap(),
            workspace.path(),
            SkillMaterializationDestination::parse("skill-creator-tmp-01/starter-skill").unwrap(),
        )
        .unwrap();
        let outcome = SkillResourceMaterializer::new()
            .materialize_template_tree(&resources, &request)
            .unwrap();
        assert_eq!(outcome.status(), SkillMaterializationStatus::Created);
        let starter = fs::read_to_string(
            workspace
                .path()
                .join("skill-creator-tmp-01/starter-skill/SKILL.md"),
        )
        .unwrap();
        let parsed = parse_skill_document(&starter, "starter-skill").unwrap();
        assert_eq!(parsed.metadata.name, "replace-with-skill-name");
        assert!(!starter[parsed.instructions_range].trim().is_empty());
    }

    #[test]
    fn office_skills_are_v3_packages_with_progressively_disclosed_resources() {
        let source = BundledSkillSource::new().unwrap();
        let catalog = source.list().unwrap();
        let runtime_manifest: Value = serde_json::from_str(include_str!(
            "../../../../resources/artifact-runtime-manifest.json"
        ))
        .unwrap();

        for (
            local_id,
            tool,
            extension,
            runtime_profile,
            template_path,
            first_entrypoint,
            first_dependency,
            first_dependency_version,
            contract_version,
        ) in [
            (
                DOCUMENTS_LOCAL_ID,
                "office_document",
                ".docx",
                "documents",
                "templates/builder.py",
                "python",
                "python-docx",
                "1.2.0",
                14,
            ),
            (
                PRESENTATIONS_LOCAL_ID,
                "office_presentation",
                ".pptx",
                "presentations",
                "templates/builder.mjs",
                "node",
                "pptxgenjs",
                "4.0.1",
                11,
            ),
            (
                SPREADSHEETS_LOCAL_ID,
                "office_spreadsheet",
                ".xlsx",
                "spreadsheets",
                "templates/builder.py",
                "python",
                "numpy",
                "2.5.2",
                12,
            ),
        ] {
            let descriptor = catalog
                .skills()
                .iter()
                .find(|skill| skill.id().local_id() == local_id)
                .unwrap();
            let package = source.resolve(&descriptor.selection()).unwrap();

            assert_eq!(package.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V3);
            assert!(package
                .revision()
                .as_str()
                .starts_with("skill-package-sha256-v3:"));
            assert_eq!(package.resources().len(), 5);
            assert_eq!(
                package.resources().entries()[0].path(),
                "office-capability.json"
            );
            assert_eq!(
                package.resources().entries()[0].kind(),
                SkillResourceKind::Other
            );
            let workflow = package
                .resources()
                .get("references/workflows.md")
                .expect("every Office Skill must expose its workflow reference");
            assert_eq!(workflow.kind(), SkillResourceKind::Reference);
            let template = package
                .resources()
                .get(template_path)
                .expect("every Office Skill must expose its builder template");
            assert_eq!(template.kind(), SkillResourceKind::Other);
            if local_id == PRESENTATIONS_LOCAL_ID {
                assert_eq!(
                    package
                        .resources()
                        .entries()
                        .iter()
                        .map(|resource| resource.path())
                        .collect::<Vec<_>>(),
                    vec![
                        "office-capability.json",
                        "references/editing-existing.md",
                        "references/workflows.md",
                        "templates/builder.mjs",
                        "templates/editor.mjs",
                    ]
                );
                assert_eq!(
                    package
                        .resources()
                        .get("references/editing-existing.md")
                        .unwrap()
                        .kind(),
                    SkillResourceKind::Reference
                );
                assert_eq!(
                    package
                        .resources()
                        .get("templates/editor.mjs")
                        .unwrap()
                        .kind(),
                    SkillResourceKind::Other
                );
                assert!(package.instructions().contains("Managed Editor"));
            }
            if local_id == DOCUMENTS_LOCAL_ID {
                assert_eq!(
                    package
                        .resources()
                        .entries()
                        .iter()
                        .map(|resource| resource.path())
                        .collect::<Vec<_>>(),
                    vec![
                        "office-capability.json",
                        "references/editing-existing.md",
                        "references/workflows.md",
                        "templates/builder.py",
                        "templates/editor.py",
                    ]
                );
                assert_eq!(
                    package
                        .resources()
                        .get("references/editing-existing.md")
                        .unwrap()
                        .kind(),
                    SkillResourceKind::Reference
                );
                assert_eq!(
                    package
                        .resources()
                        .get("templates/editor.py")
                        .unwrap()
                        .kind(),
                    SkillResourceKind::Other
                );
                assert!(package.instructions().contains("Word Editor"));
            }
            if local_id == SPREADSHEETS_LOCAL_ID {
                assert_eq!(
                    package
                        .resources()
                        .entries()
                        .iter()
                        .map(|resource| resource.path())
                        .collect::<Vec<_>>(),
                    vec![
                        "office-capability.json",
                        "references/editing-existing.md",
                        "references/workflows.md",
                        "templates/builder.py",
                        "templates/editor.py",
                    ]
                );
                assert_eq!(
                    package
                        .resources()
                        .get("references/editing-existing.md")
                        .unwrap()
                        .kind(),
                    SkillResourceKind::Reference
                );
                assert_eq!(
                    package
                        .resources()
                        .get("templates/editor.py")
                        .unwrap()
                        .kind(),
                    SkillResourceKind::Other
                );
                assert!(package.instructions().contains("Workbook Editor"));
            }
            assert!(package.instructions().contains(tool));
            assert!(package.instructions().contains("flat semantic"));
            assert!(package.instructions().contains("Managed Builder"));
            assert!(package.instructions().contains("runtimeProfile"));
            assert!(package.instructions().contains("run_command.inputs"));
            assert!(package.instructions().contains("MYCOPILOT_INPUT_ROOT"));
            assert!(package.instructions().contains("static `--output`"));
            assert!(package.instructions().contains("artifactObservation"));
            assert!(package.instructions().contains("outputs[].readPath"));
            assert!(package.instructions().contains("read_image.path"));

            let reader = source.open_resource_reader(&package).unwrap().unwrap();
            let capability = reader.read(&package.resources().entries()[0]).unwrap();
            let capability: serde_json::Value = serde_json::from_slice(&capability).unwrap();
            assert_eq!(capability["contractVersion"], contract_version);
            assert_eq!(capability["engine"], "officecli");
            assert_eq!(capability["tool"], tool);
            assert_eq!(capability["extensions"][0], extension);
            assert_eq!(capability["modes"]["native"]["tool"], tool);
            assert_eq!(
                capability["modes"]["native"]["requestStyle"],
                "flat_semantic_v1"
            );
            assert_eq!(capability["modes"]["native"]["reasonField"], "reason");
            assert_eq!(capability["modes"]["native"]["reasonRequired"], true);
            assert_eq!(
                capability["modes"]["native"]["providerArguments"],
                "host_generated"
            );
            assert_eq!(
                capability["modes"]["native"]["unsupportedRecovery"],
                "useManagedScript"
            );
            let render_output = &capability["modes"]["native"]["renderOutput"];
            assert_eq!(render_output["resultField"], "outputs");
            assert_eq!(render_output["role"], "render");
            assert_eq!(render_output["kind"], "image");
            assert_eq!(render_output["pathField"], "readPath");
            assert_eq!(render_output["consumerTool"], "read_image");
            assert_eq!(render_output["consumerField"], "path");
            assert_eq!(render_output["readabilityField"], "readableByAgent");
            assert_eq!(render_output["sizeField"], "sizeBytes");
            assert_eq!(render_output["selectionField"], "pageSelection");
            assert_eq!(
                render_output["selectionSemantics"],
                "requested_not_verified_coverage"
            );
            assert_eq!(render_output["pathInference"], "forbidden");
            let script = &capability["modes"]["script"];
            assert_eq!(script["builderTemplate"], template_path);
            assert_eq!(script["executionTool"], "run_command");
            assert_eq!(script["runtimeProfile"], runtime_profile);
            assert_eq!(
                script["runtimeProfileBinding"],
                "host_from_materialized_script_receipt"
            );
            assert_eq!(script["outputArgument"], "--output");
            assert!(script.get("runtimeProfileField").is_none());
            assert_eq!(script["entrypoints"][0]["command"], first_entrypoint);
            assert_eq!(
                script["entrypoints"][0]["dependencies"][0]["name"],
                first_dependency
            );
            assert_eq!(
                script["entrypoints"][0]["dependencies"][0]["version"],
                first_dependency_version
            );
            assert_eq!(script["resolution"]["authority"], "host");
            assert_eq!(script["resolution"]["runtimeKindFrom"], "command");
            assert_eq!(script["resolution"]["dependencyPolicy"], "profilePinned");
            assert!(script.get("runtimes").is_none());
            assert!(script.get("requiredPackagesField").is_none());
            assert_eq!(script["inputs"]["field"], "inputs");
            assert_eq!(script["inputs"]["mountPathField"], "mountPath");
            assert_eq!(script["inputs"]["pathField"], "path");
            assert_eq!(script["inputs"]["rootEnvironment"], "MYCOPILOT_INPUT_ROOT");
            assert_eq!(
                script["inputs"]["pathKinds"],
                serde_json::json!([
                    "workspace",
                    "absolute_or_system",
                    "attachment_read_path",
                    "generated_artifact_uri",
                    "revision_bound_skill_uri"
                ])
            );
            assert_eq!(script["observation"]["requiredFromModel"], false);
            assert_eq!(
                script["observation"]["binding"],
                "host_from_materialization_and_output_argument"
            );
            assert_eq!(script["observation"]["kind"], "office");
            assert!(script["observation"].get("expectedOutputsField").is_none());
            assert_eq!(
                script["observation"]["additionalRootsField"],
                "observe.additionalRoots"
            );
            assert_eq!(script["observation"]["resultField"], "artifactObservation");
            for entrypoint in script["entrypoints"].as_array().unwrap() {
                let command = entrypoint["command"].as_str().unwrap();
                let runtime = if command == "node" { "node" } else { "python" };
                assert_eq!(
                    entrypoint["runtimeVersion"],
                    runtime_manifest[runtime]["version"]
                );
                for dependency in entrypoint["dependencies"].as_array().unwrap() {
                    let expected_name = dependency["name"].as_str().unwrap();
                    let expected_version = dependency["version"].as_str().unwrap();
                    assert!(
                        runtime_manifest[runtime]["dependencies"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|resolved| {
                                resolved["name"] == expected_name
                                    && resolved["version"] == expected_version
                            }),
                        "{local_id} declares {expected_name}@{expected_version}, which is not pinned by the managed Artifact Runtime"
                    );
                }
            }
        }
    }

    #[test]
    fn document_and_spreadsheet_skills_define_second_round_python_script_contracts() {
        let source = BundledSkillSource::new().unwrap();
        for (
            local_id,
            workflow_source,
            directory_command,
            materialized_destination,
            template_path,
        ) in [
            (
                DOCUMENTS_LOCAL_ID,
                include_str!("bundled/documents/references/workflows.md"),
                "\"command\": \"mkdir -p scripts\"",
                "\"destination\": \"scripts/build_report.py\"",
                "templates/builder.py",
            ),
            (
                SPREADSHEETS_LOCAL_ID,
                include_str!("bundled/spreadsheets/references/workflows.md"),
                "\"command\": \"mkdir -p workbook-work\"",
                "\"destination\": \"workbook-work/build_workbook.py\"",
                "templates/builder.py",
            ),
        ] {
            let descriptor = source
                .list()
                .unwrap()
                .skills()
                .iter()
                .find(|skill| skill.id().local_id() == local_id)
                .unwrap()
                .clone();
            let package = source.resolve(&descriptor.selection()).unwrap();
            let reader = source.open_resource_reader(&package).unwrap().unwrap();
            let capability = reader.read(&package.resources().entries()[0]).unwrap();
            let capability: Value = serde_json::from_slice(&capability).unwrap();
            assert_eq!(
                capability["contractVersion"],
                if local_id == DOCUMENTS_LOCAL_ID {
                    14
                } else {
                    12
                }
            );

            let script = &capability["modes"]["script"];
            assert_eq!(script["entrypoints"].as_array().unwrap().len(), 1);
            assert_eq!(script["entrypoints"][0]["command"], "python");
            assert_eq!(
                capability["modes"]["native"]["operations"],
                serde_json::json!(["inspect", "render"])
            );
            assert_eq!(
                capability["modes"]["native"]["writeOperationsModelVisible"],
                false
            );
            let expected_lifecycle = if local_id == DOCUMENTS_LOCAL_ID {
                serde_json::json!({
                    "prepareDirectoryBeforeMaterialize": true,
                    "defaultOutputScope": "workspaceRoot",
                    "nestedOutputParentMustExistBeforeRun": true,
                    "reuseSingleScript": true,
                    "cleanupTaskOwnedFiles": true,
                    "removeDirectoryOnlyIfTaskCreatedAndEmpty": true,
                    "recursiveDelete": "forbidden"
                })
            } else {
                serde_json::json!({
                    "prepareDirectoryBeforeMaterialize": true,
                    "reuseSingleScript": true,
                    "cleanupTaskOwnedFiles": true,
                    "removeDirectoryOnlyIfTaskCreatedAndEmpty": true,
                    "recursiveDelete": "forbidden"
                })
            };
            assert_eq!(script["lifecycle"], expected_lifecycle);
            assert_eq!(
                script["syntaxPreflight"],
                serde_json::json!({
                    "binding": "hostAutomaticBeforeExecution",
                    "language": "python",
                    "modelCommand": "forbidden",
                    "invalidatesOnScriptChange": true,
                    "executionOnFailure": "forbidden"
                })
            );

            let prepare = workflow_source.find(directory_command).unwrap();
            let materialize = workflow_source.find(materialized_destination).unwrap();
            assert!(
                prepare < materialize,
                "{local_id} must prepare the selected directory before materialization"
            );
            for required in [
                "Host",
                "preflight",
                "task-created",
                "rmdir",
                "recursive deletion",
            ] {
                assert!(
                    workflow_source.contains(required),
                    "{local_id} workflow is missing `{required}`"
                );
            }

            let session = SkillsService::new().with_bundled_source().unwrap();
            let activated = session.activate(&[descriptor.selection()]).unwrap();
            let resource_session = session.resource_session(&activated).unwrap();
            let package_uri = resource_session.package_uris().into_iter().next().unwrap();
            let template_uri =
                package_uri.resource(SkillResourcePath::parse(template_path).unwrap());
            let workspace = tempdir().unwrap();
            let destination = materialized_destination
                .trim_start_matches("\"destination\": \"")
                .trim_end_matches('"');
            fs::create_dir(
                workspace
                    .path()
                    .join(Path::new(destination).parent().unwrap()),
            )
            .unwrap();
            let request = SkillMaterializationRequest::new(
                template_uri,
                workspace.path(),
                SkillMaterializationDestination::parse(destination).unwrap(),
            )
            .unwrap();
            let outcome = SkillResourceMaterializer::new()
                .materialize(&resource_session, &request)
                .unwrap();
            assert_eq!(outcome.status(), SkillMaterializationStatus::Created);
            let materialized = workspace.path().join(destination);
            assert!(materialized.is_file());
            if local_id == SPREADSHEETS_LOCAL_ID {
                let builder = fs::read_to_string(materialized).unwrap();
                assert!(builder.contains("from openpyxl import Workbook"));
                assert!(builder.contains("workbook.save(output)"));
                assert!(!builder.contains("load_workbook"));
                assert!(!builder.contains("tempfile"));
                assert!(!builder.contains("os.replace"));
            } else {
                let builder = fs::read_to_string(materialized).unwrap();
                assert!(builder.contains("document.save(output)"));
                assert!(!builder.contains("Document(temporary)"));
                assert!(!builder.contains("tempfile"));
                assert!(!builder.contains("os.replace"));
            }
        }

        let documents: Value =
            serde_json::from_str(include_str!("bundled/documents/office-capability.json")).unwrap();
        assert_eq!(
            documents["modes"]["script"]["routes"],
            serde_json::json!({
                "create": "builder",
                "editExisting": "editor",
                "inspectRender": "native"
            })
        );
        assert_eq!(documents["validation"]["inspectFinal"], true);
        assert_eq!(
            documents["validation"]["packageValidation"],
            "hostPrepublishPinnedOfficeCliSchemaGate"
        );
        assert_eq!(documents["validation"]["modelNativeValidate"], "forbidden");
        assert_eq!(
            documents["validation"]["pageCountSource"],
            "pdfSkillPdfinfoOnExactReturnedPdf"
        );
        assert_eq!(
            documents["validation"]["authoritativePageCountAvailable"],
            true
        );
        assert_eq!(
            documents["validation"]["visual"]["pageRangeRequests"],
            "pdfSkillOneThroughN"
        );
        assert_eq!(
            documents["validation"]["visual"]["maxPagesPerRasterBatch"],
            32
        );
        assert_eq!(
            documents["validation"]["temporaryArtifactCleanup"]["managedPdfPageImages"],
            "hostRunWorkspaceCleanupAfterRead"
        );
        assert_eq!(
            documents["modes"]["native"]["pdfQaOutput"]["requestRequiredFields"],
            serde_json::json!([
                "operation",
                "filePath",
                "outputPath",
                "outputFormat",
                "reason"
            ])
        );
        assert_eq!(
            documents["modes"]["native"]["pdfQaOutput"]["pageCountField"],
            "outputs[].pageCount"
        );
        assert_eq!(
            documents["modes"]["native"]["pdfQaOutput"]["oneCallPerFinalDocumentRevision"],
            true
        );
        assert_eq!(
            documents["modes"]["native"]["pdfQaOutput"]["requiredPdfOutputFields"],
            serde_json::json!(["readPath", "pageCount", "sourceSha256", "rendererRevision"])
        );
        assert_eq!(
            documents["modes"]["native"]["pdfQaOutput"]["sourceRevisionField"],
            "outputs[].sourceSha256"
        );
        assert_eq!(
            documents["modes"]["native"]["pdfQaOutput"]["rendererRevisionField"],
            "outputs[].rendererRevision"
        );
        assert_eq!(
            documents["modes"]["native"]["pdfQaOutput"]["outputPathScope"],
            "existingTaskScriptOrTemporaryDirectory"
        );
        assert_eq!(
            documents["modes"]["native"]["pdfQaOutput"]["outputPathKind"],
            "workspaceRelative"
        );
        assert_eq!(
            documents["modes"]["native"]["pdfQaOutput"]["topLevelOutputsDirectory"],
            "forbidden"
        );
        assert_eq!(
            documents["modes"]["native"]["pdfQaOutput"]["handoff"]["runtimeEnforced"],
            false
        );
        assert_eq!(
            documents["modes"]["native"]["pdfQaOutput"]["handoff"]["sourcePathMode"],
            "exactDirect"
        );
        assert_eq!(
            documents["modes"]["native"]["pdfQaOutput"]["handoff"]["sourcePathKind"],
            "workspaceRelative"
        );
        assert_eq!(
            documents["modes"]["native"]["pdfQaOutput"]["handoff"]["runCommandInputsRequired"],
            false
        );
        assert_eq!(
            documents["modes"]["script"]["lifecycle"]["defaultOutputScope"],
            "workspaceRoot"
        );
        assert_eq!(
            documents["modes"]["script"]["lifecycle"]["nestedOutputParentMustExistBeforeRun"],
            true
        );
        assert_eq!(
            documents["modes"]["script"]["exceptionalFallback"]["scope"],
            serde_json::json!(["create", "editExisting"])
        );
        assert_eq!(
            documents["modes"]["script"]["exceptionalFallback"]["runtimeProfileRequired"],
            "documents"
        );
        assert_eq!(
            documents["modes"]["script"]["exceptionalFallback"]["visualQaConversion"],
            "managedOnly"
        );
        assert_eq!(
            documents["modes"]["script"]["exceptionalFallback"]["forbiddenVisualQaConverters"],
            serde_json::json!([
                "reportlabReconstruction",
                "pageImageStitching",
                "modelAuthoredDocxToPdf"
            ])
        );
        assert_eq!(
            documents["validation"]["conversionAndPdfinfoMustAgree"],
            true
        );
        assert_eq!(
            documents["validation"]["visual"]["fieldChecks"]["PAGE"],
            "visibleAndCorrectForDocumentNumberingRules"
        );
        assert_eq!(
            documents["validation"]["visual"]["fieldChecks"]["NUMPAGES"],
            "visibleAndEqualsAuthoritativeN"
        );
        assert_eq!(
            documents["validation"]["evidenceInvalidation"]["invalidates"],
            serde_json::json!(["temporaryPdf", "pageCount", "pageImages", "pageLedger"])
        );
        assert_eq!(
            documents["validation"]["temporaryArtifactCleanup"]["qaPdfIsFinalArtifact"],
            false
        );
        assert_eq!(
            documents["modes"]["native"]["renderOutput"]["finalVisualQaEvidence"],
            "forbidden"
        );
        assert_eq!(
            documents["validation"]["structuralChecks"]["renderingSufficient"],
            false
        );

        let spreadsheets: Value =
            serde_json::from_str(include_str!("bundled/spreadsheets/office-capability.json"))
                .unwrap();
        assert_eq!(
            spreadsheets["modes"]["script"]["routes"],
            serde_json::json!({
                "create": "builder",
                "editExisting": "editor",
                "inspectRender": "native",
                "nativeWrite": "forbidden"
            })
        );
        assert_eq!(spreadsheets["calculation"]["recalculation"], "notPerformed");
        assert_eq!(
            spreadsheets["validation"]["packageValidation"],
            "hostPrepublishPinnedOpenpyxlReopen"
        );
        assert_eq!(
            spreadsheets["modes"]["script"]["transaction"]["validation"]["arguments"]["data_only"],
            false
        );
        assert_eq!(
            spreadsheets["validation"]["visual"]["coverage"],
            "everyResolvableFinalSheetVisualExtent"
        );
        assert_eq!(
            spreadsheets["validation"]["visual"]["visualExtent"],
            "populatedUsedRangeUnionReportedChartAndImageBounds"
        );
        assert_eq!(
            spreadsheets["validation"]["visual"]["unresolvedFloatingObjectBounds"],
            "discloseIncompleteCoverage"
        );
    }

    #[test]
    fn documents_skill_exposes_one_full_python_existing_document_editor() {
        let workflows = include_str!("bundled/documents/references/workflows.md");
        let editing = include_str!("bundled/documents/references/editing-existing.md");
        let editor = include_str!("bundled/documents/templates/editor.py");
        let capability: Value =
            serde_json::from_str(include_str!("bundled/documents/office-capability.json")).unwrap();

        for required in [
            "**Edit an existing `.docx`:**",
            "`templates/editor.py`",
            "one mounted source and one distinct save-as output",
            "never substitute native write calls",
            "The Word Editor executes normal Python",
            "it is not an AST or",
            "Host automatically performs an isolated Python syntax preflight",
            "wait with `command_session`",
            "never use recursive deletion",
            "activate and follow the PDF Skill",
            "`outputFormat: \"pdf\"`",
            "model-authored DOCX-to-PDF converters are",
            "`outputs[].sourceSha256`",
            "`outputs[].rendererRevision`",
            "batches of at most 32 pages",
            "verdict per page",
            "The QA PDF is temporary evidence, not a final artifact",
            "never place that PDF in the workspace's\ntop-level `outputs/` directory",
            "`outputPath` must be\nworkspace-relative",
            "never pass an absolute path",
            "do not add `run_command.inputs` or rewrite it",
            "Prefer a workspace-root final output",
            "`PAGE` or `NUMPAGES`",
        ] {
            assert!(
                DOCUMENTS_SOURCE.contains(required),
                "document instructions are missing `{required}`"
            );
        }
        assert!(!DOCUMENTS_SOURCE.contains("--output outputs/"));
        assert!(!workflows.contains("--output outputs/"));
        assert!(!workflows.contains("\"outputPath\": \"outputs/"));

        for required in [
            "Create every new document with the Managed\nBuilder",
            "Edit every existing document with the fixed Managed Editor",
            "[editing-existing.md](editing-existing.md)",
            "Use `office_document` only for `inspect` and `render`",
            "Host's pinned OfficeCLI schema gate",
            "stable OfficeCLI `type`",
            "`description`, `path`, `part`, `code`, `error`, and `message` fields",
            "## Use exceptional Python fallback",
            "## Convert once and follow the PDF Skill",
            "`outputPath` must be workspace-relative; an absolute path is invalid",
            "Use `pdfinfo` to obtain authoritative page count",
            "python scripts/build_report.py --output report.docx",
            "must already exist before `run_command`",
            "do not add\n`run_command.inputs`",
            "pdfinfo \"<exact outputs[].readPath>\"",
            "outputs/report-page",
            "Render all pages `1..N` in contiguous",
            "outputs[].sourceSha256",
            "outputs[].rendererRevision",
            "Bind the visual evidence ledger to `sourceSha256`",
            "batches of at most 32 pages",
            "consume every exact returned page-image path with\n`read_image.path`",
            "Do not use ReportLab, image stitching, or a self-authored converter",
            "Do not present the QA PDF as a\nfinal artifact",
            "## Clean up exact task artifacts",
        ] {
            assert!(
                workflows.contains(required),
                "document workflow is missing `{required}`"
            );
        }

        for required in [
            "python word-work/edit_document.py --source source.docx --output report-edited.docx",
            "`source.docx` is a logical mount",
            "The Editor is real Python, not a serialized edit plan",
            "functions, imports from the pinned runtime, loops, conditions",
            "`mounted_input(\"media/replacement.png\")`",
            "existing managed-command permission and approval boundary",
            "does not publish its private candidate to the declared destination before every gate passes",
            "does not make unrelated side effects\ntransactional",
            "Fail closed instead of silently flattening, rebuilding, or approximating",
            "raw OOXML only when the user explicitly requests that route",
        ] {
            assert!(
                editing.contains(required),
                "existing-document editing reference is missing `{required}`"
            );
        }

        assert_eq!(editor.matches("# BEGIN EDIT REGION").count(), 1);
        assert_eq!(editor.matches("# END EDIT REGION").count(), 1);
        let begin = editor.find("# BEGIN EDIT REGION").unwrap();
        let end = editor.find("# END EDIT REGION").unwrap();
        assert!(begin < end, "the Editor edit region must be ordered");
        let edit_region = &editor[begin..end];
        for required in ["for paragraph", "if \"Old text\"", "for run"] {
            assert!(
                edit_region.contains(required),
                "Editor example is missing normal Python construct `{required}`"
            );
        }
        for required in [
            "from docx import Document",
            "def mounted_input(mount_path: str) -> Path:",
            "MYCOPILOT_INPUT_ROOT",
            "candidate = (root / relative).resolve(strict=True)",
            "def edit_document(document, source_path: Path) -> None:",
            "document = Document(source)",
            "edit_document(document, source)",
            "Replace the EDIT REGION with the requested document edits",
            "document.save(output)",
        ] {
            assert!(editor.contains(required), "Editor is missing `{required}`");
        }
        for forbidden in ["ast.parse", "json plan", "OfficeOperationParameters"] {
            assert!(
                !editor.contains(forbidden),
                "full Python Editor must not compile a restricted `{forbidden}` plan"
            );
        }

        assert!(!editor.contains("Document(temporary)"));
        assert!(!editor.contains("tempfile"));
        assert!(!editor.contains("os.replace"));

        assert_eq!(capability["contractVersion"], 14);
        let native = &capability["modes"]["native"];
        assert_eq!(
            native["operations"],
            serde_json::json!(["inspect", "render"])
        );
        assert_eq!(native["writeOperationsModelVisible"], false);
        let script = &capability["modes"]["script"];
        assert_eq!(script["builderTemplate"], "templates/builder.py");
        assert_eq!(script["editorTemplate"], "templates/editor.py");
        assert_eq!(script["editingReference"], "references/editing-existing.md");
        assert_eq!(script["routes"]["create"], "builder");
        assert_eq!(script["routes"]["editExisting"], "editor");
        assert_eq!(
            script["runtimeProfileBinding"],
            "host_from_materialized_script_receipt"
        );
        let editor_contract = &script["editor"];
        assert_eq!(editor_contract["sourceArgument"], "--source");
        assert_eq!(editor_contract["outputArgument"], "--output");
        assert_eq!(editor_contract["sourceBinding"], "inputs.mountPath");
        assert_eq!(editor_contract["defaultPublishMode"], "saveAs");
        assert_eq!(
            editor_contract["supportedPublishModes"],
            serde_json::json!(["saveAs"])
        );
        assert_eq!(editor_contract["inPlace"], false);
        assert_eq!(editor_contract["modelCode"]["execution"], "normalPython");
        assert_eq!(editor_contract["modelCode"]["astOrJsonDsl"], false);
        assert_eq!(
            editor_contract["modelCode"]["supports"],
            serde_json::json!([
                "pinnedImports",
                "functions",
                "loops",
                "conditions",
                "comprehensions",
                "dataTransformations",
                "pythonDocxObjectModel"
            ])
        );
        assert_eq!(editor_contract["transaction"]["candidate"], "hostPrivate");
        assert_eq!(editor_contract["fixedWrapper"]["candidateReopen"], false);
        assert_eq!(editor_contract["fixedWrapper"]["temporarySave"], false);
        assert_eq!(
            editor_contract["fixedWrapper"]["candidateValidationOwner"],
            "hostPinnedOfficeCliSchemaGate"
        );
        assert_eq!(
            editor_contract["fixedWrapper"]["scriptOutputWrite"],
            "directToHostPrivateCandidate"
        );
        assert_eq!(
            editor_contract["transaction"]["packageValidation"]["engine"],
            "pinnedOfficeCli"
        );
        assert_eq!(
            editor_contract["transaction"]["packageValidation"]["failureDiagnostics"]
                ["providerStableFields"],
            serde_json::json!([
                "type",
                "description",
                "path",
                "part",
                "code",
                "error",
                "message"
            ])
        );
        assert_eq!(editor_contract["transaction"]["publication"], "atomic");
        assert_eq!(
            editor_contract["transaction"]["failureBeforePublish"],
            "hostCandidateNotPublishedToDeclaredDestination"
        );

        let service = SkillsService::new().with_bundled_source().unwrap();
        let descriptor = service
            .list()
            .unwrap()
            .skills()
            .iter()
            .find(|skill| skill.id().local_id() == DOCUMENTS_LOCAL_ID)
            .unwrap()
            .clone();
        let activated = service.activate(&[descriptor.selection()]).unwrap();
        let resources = service.resource_session(&activated).unwrap();
        let package = resources.package_uris().into_iter().next().unwrap();
        let editor_uri = package.resource(SkillResourcePath::parse("templates/editor.py").unwrap());
        let materialized_source = resources
            .read_text(&editor_uri, SkillResourceTextReadOptions::default())
            .unwrap();
        assert_eq!(materialized_source.text(), editor);
        let workspace = tempdir().unwrap();
        fs::create_dir(workspace.path().join("word-work")).unwrap();
        let request = SkillMaterializationRequest::new(
            editor_uri,
            workspace.path(),
            SkillMaterializationDestination::parse("word-work/edit_document.py").unwrap(),
        )
        .unwrap();
        let outcome = SkillResourceMaterializer::new()
            .materialize(&resources, &request)
            .unwrap();
        assert_eq!(outcome.status(), SkillMaterializationStatus::Created);
        assert_eq!(
            fs::read_to_string(workspace.path().join("word-work/edit_document.py")).unwrap(),
            editor
        );
    }

    #[test]
    fn spreadsheets_skill_exposes_one_full_python_existing_workbook_editor() {
        let workflows = include_str!("bundled/spreadsheets/references/workflows.md");
        let editing = include_str!("bundled/spreadsheets/references/editing-existing.md");
        let editor = include_str!("bundled/spreadsheets/templates/editor.py");
        let capability: Value =
            serde_json::from_str(include_str!("bundled/spreadsheets/office-capability.json"))
                .unwrap();

        for required in [
            "**Edit an existing `.xlsx`:**",
            "`templates/editor.py`",
            "executes normal Python against the frozen source snapshot",
            "Never convert it into an AST, JSON, or artificial operation DSL",
            "automatic syntax preflight",
            "wait on\nthat same command session",
            "never use recursive deletion",
        ] {
            assert!(
                SPREADSHEETS_SOURCE.contains(required),
                "spreadsheet instructions are missing `{required}`"
            );
        }
        for required in [
            "Use the fixed Python Managed Builder or Editor for every write",
            "materialize `templates/editor.py` to edit an existing `.xlsx`",
            "patch only `edit_workbook`",
            "The edit region is normal Python",
            "private staging",
            "pinned `openpyxl.load_workbook(data_only=False)`",
            "publishes atomically",
            "not a claim that unrestricted Python is\na separate cross-platform OS sandbox",
        ] {
            assert!(
                workflows.contains(required),
                "spreadsheet workflow is missing `{required}`"
            );
        }
        for required in [
            "python workbook-work/edit_workbook.py --source source.xlsx --output workbook-edited.xlsx",
            "The Editor executes\nnormal Python",
            "helper functions, loops, conditions, comprehensions",
            "existing\n`run_command` permission and approval boundary",
            "redirects the intended output to private\nstaging",
            "does not pretend unrestricted Python is a separate",
            "cross-platform OS sandbox",
            "Stop rather than silently degrade",
        ] {
            assert!(
                editing.contains(required),
                "existing-workbook editing reference is missing `{required}`"
            );
        }

        assert_eq!(editor.matches("# BEGIN EDIT REGION").count(), 1);
        assert_eq!(editor.matches("# END EDIT REGION").count(), 1);
        let begin = editor.find("# BEGIN EDIT REGION").unwrap();
        let end = editor.find("# END EDIT REGION").unwrap();
        assert!(begin < end, "the Editor edit region must be ordered");
        let edit_region = &editor[begin..end];
        for required in ["for row in range", "sheet.cell", "f\"=SUM("] {
            assert!(
                edit_region.contains(required),
                "Editor example is missing normal Python construct `{required}`"
            );
        }
        for required in [
            "from openpyxl import load_workbook",
            "def mounted_input(mount_path: str) -> tuple[Path, Path]:",
            "MYCOPILOT_INPUT_ROOT",
            "def edit_workbook(workbook, input_root: Path) -> None:",
            "data_only=False",
            "keep_links=True",
            "rich_text=True",
            "workbook.save(output)",
        ] {
            assert!(editor.contains(required), "Editor is missing `{required}`");
        }
        for forbidden in ["ast.parse", "json plan", "OfficeOperationParameters"] {
            assert!(
                !editor.contains(forbidden),
                "full Python Editor must not compile a restricted `{forbidden}` plan"
            );
        }

        assert!(!editor.contains("verified = load_workbook"));
        assert!(!editor.contains("tempfile"));
        assert!(!editor.contains("os.replace"));

        assert_eq!(capability["contractVersion"], 12);
        assert_eq!(
            capability["modes"]["native"]["operations"],
            serde_json::json!(["inspect", "render"])
        );
        assert_eq!(
            capability["modes"]["native"]["writeOperationsModelVisible"],
            false
        );
        let script = &capability["modes"]["script"];
        assert_eq!(script["builderTemplate"], "templates/builder.py");
        assert_eq!(script["editorTemplate"], "templates/editor.py");
        assert_eq!(script["editingReference"], "references/editing-existing.md");
        assert_eq!(script["routes"]["create"], "builder");
        assert_eq!(script["routes"]["editExisting"], "editor");
        assert_eq!(script["routes"]["nativeWrite"], "forbidden");
        assert_eq!(script["routes"]["inspectRender"], "native");
        assert_eq!(
            script["runtimeProfileBinding"],
            "host_from_materialized_script_receipt"
        );
        assert_eq!(script["pythonSemantics"]["execution"], "normalPython");
        assert_eq!(script["pythonSemantics"]["astOrJsonDsl"], "forbidden");
        for field in ["functions", "loops", "conditions", "comprehensions"] {
            assert_eq!(script["pythonSemantics"][field], true);
        }
        assert_eq!(
            script["transaction"]["candidateOutput"],
            "hostPrivateStaging"
        );
        assert_eq!(
            script["transaction"]["scriptOutputWrite"],
            "directToHostPrivateCandidate"
        );
        assert_eq!(script["transaction"]["publish"], "atomic");
        assert_eq!(
            script["transaction"]["failureBeforePublish"],
            "hostCandidateNotPublishedToDeclaredDestination"
        );
        assert_eq!(
            script["transaction"]["pythonSideEffectScope"],
            "outsideOfficePublicationNotTransactional"
        );
        assert_eq!(script["transaction"]["crossPlatformOsSandboxClaim"], false);
        assert_eq!(
            script["transaction"]["validation"]["engine"],
            "pinnedOpenpyxl"
        );
        assert_eq!(
            script["transaction"]["validation"]["operation"],
            "load_workbook"
        );
        assert_eq!(
            script["transaction"]["validation"]["arguments"]["data_only"],
            false
        );
        assert_eq!(
            script["entrypoints"][0]["dependencies"],
            serde_json::json!([
                { "name": "numpy", "version": "2.5.2" },
                { "name": "openpyxl", "version": "3.1.5" },
                { "name": "pandas", "version": "3.0.5" },
                { "name": "xlsxwriter", "version": "3.2.9" }
            ])
        );

        let service = SkillsService::new().with_bundled_source().unwrap();
        let descriptor = service
            .list()
            .unwrap()
            .skills()
            .iter()
            .find(|skill| skill.id().local_id() == SPREADSHEETS_LOCAL_ID)
            .unwrap()
            .clone();
        let activated = service.activate(&[descriptor.selection()]).unwrap();
        let resources = service.resource_session(&activated).unwrap();
        let package = resources.package_uris().into_iter().next().unwrap();
        let editor_uri = package.resource(SkillResourcePath::parse("templates/editor.py").unwrap());
        let materialized_source = resources
            .read_text(&editor_uri, SkillResourceTextReadOptions::default())
            .unwrap();
        assert_eq!(materialized_source.text(), editor);
        let workspace = tempdir().unwrap();
        fs::create_dir(workspace.path().join("workbook-work")).unwrap();
        let request = SkillMaterializationRequest::new(
            editor_uri,
            workspace.path(),
            SkillMaterializationDestination::parse("workbook-work/edit_workbook.py").unwrap(),
        )
        .unwrap();
        let outcome = SkillResourceMaterializer::new()
            .materialize(&resources, &request)
            .unwrap();
        assert_eq!(outcome.status(), SkillMaterializationStatus::Created);
        assert_eq!(
            fs::read_to_string(workspace.path().join("workbook-work/edit_workbook.py")).unwrap(),
            editor
        );
    }

    #[test]
    fn office_skill_native_json_examples_use_flat_semantic_requests() {
        let documents = [
            ("documents/SKILL.md", DOCUMENTS_SOURCE),
            (
                "documents/references/workflows.md",
                include_str!("bundled/documents/references/workflows.md"),
            ),
            ("spreadsheets/SKILL.md", SPREADSHEETS_SOURCE),
            (
                "spreadsheets/references/workflows.md",
                include_str!("bundled/spreadsheets/references/workflows.md"),
            ),
            ("presentations/SKILL.md", PRESENTATIONS_SOURCE),
            (
                "presentations/references/workflows.md",
                include_str!("bundled/presentations/references/workflows.md"),
            ),
        ];
        let mut native_example_count = 0;

        for (document_name, markdown) in documents {
            for (line_number, example) in markdown_json_examples(document_name, markdown) {
                let Some(object) = example.as_object() else {
                    continue;
                };
                let Some(operation) = object.get("operation").and_then(Value::as_str) else {
                    continue;
                };
                native_example_count += 1;
                assert!(
                    !operation.trim().is_empty(),
                    "{document_name}:{line_number} native Office operation must not be empty"
                );
                assert!(
                    !object.contains_key("request"),
                    "{document_name}:{line_number} native Office example must use the flat semantic surface"
                );
                assert!(
                    !object.contains_key("arguments"),
                    "{document_name}:{line_number} native Office example must not expose provider arguments"
                );
                assert!(
                    !object.contains_key("target")
                        && !object.contains_key("parent")
                        && !object.contains_key("element")
                        && !object.contains_key("properties"),
                    "{document_name}:{line_number} native Office example must not expose low-level DOM fields"
                );

                let reason = object
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!(
                            "{document_name}:{line_number} native Office example must include a string reason"
                        )
                    });
                assert_eq!(
                    reason,
                    reason.trim(),
                    "{document_name}:{line_number} reason must not have surrounding whitespace"
                );
                assert!(
                    !reason.is_empty(),
                    "{document_name}:{line_number} reason must not be empty"
                );
                assert!(
                    reason.chars().count() <= crate::AGENT_OFFICE_REASON_MAX_CHARS,
                    "{document_name}:{line_number} reason exceeds the Office limit"
                );
                assert!(
                    !reason.chars().any(char::is_control),
                    "{document_name}:{line_number} reason contains a control character"
                );
                assert!(
                    !reason.chars().any(is_bidirectional_text_control),
                    "{document_name}:{line_number} reason contains a bidirectional text control"
                );
            }
        }

        assert!(
            native_example_count > 0,
            "the bundled Office Skill documents must include flat native semantic examples"
        );
    }

    #[test]
    fn presentations_skill_requires_separate_builder_syntax_preflight() {
        let workflows = include_str!("bundled/presentations/references/workflows.md");

        for required in [
            "Immediately after materializing or modifying any `.mjs` Builder",
            "mkdir -p <parents...>",
            "the parent of a nested `--output`, and every\nrender `outputPath` parent",
            "`mkdir` inside Builder code is too late",
            "delete the exact Builder or Editor and any task-created temporary files",
            "never use recursive deletion for this cleanup",
            "`node --check <builder>.mjs` as a separate `run_command` call",
            "Never combine the check and build with `&&`, `|`, or `;`",
            "Any later edit invalidates the successful check",
            "Every Builder build command must declare",
            "repair the Builder source, input bindings, or provenance/materialization state",
            "Never retry the same failing Builder command unchanged",
            "`syntax-valid != runtime-valid != PPTX-valid`",
        ] {
            assert!(
                PRESENTATIONS_SOURCE.contains(required),
                "presentation instructions are missing `{required}`"
            );
        }

        for required in [
            "\"command\": \"mkdir -p scripts outputs\"",
            "Create all missing parents before the\nfirst materialization or write",
            "`mkdir` inside Builder code is too late",
            "## Clean up managed scripts",
            "Never use `rm -rf` for this cleanup",
            "\"command\": \"node --check scripts/build_deck.mjs\"",
            "If the check exits\nnon-zero, do not build",
            "Editing the file after a successful\ncheck invalidates that result",
            "Every Builder build command must declare",
            "Repeat the syntax check\nbefore the next build even when the source did not change",
            "Never retry the same failing Builder\ncommand unchanged",
            "`syntax-valid != runtime-valid != PPTX-valid`",
        ] {
            assert!(
                workflows.contains(required),
                "presentation workflow is missing `{required}`"
            );
        }

        assert!(!PRESENTATIONS_SOURCE.contains("Every Builder command must declare"));
        assert!(!workflows.contains("Every Builder command must declare"));

        let check = workflows
            .find("\"command\": \"node --check scripts/build_deck.mjs\"")
            .unwrap();
        let prepare = workflows
            .find("\"command\": \"mkdir -p scripts outputs\"")
            .unwrap();
        let materialize = workflows
            .find("\"destination\": \"scripts/build_deck.mjs\"")
            .unwrap();
        let build = workflows
            .find("\"command\": \"node scripts/build_deck.mjs --output")
            .unwrap();
        assert!(
            prepare < materialize,
            "script directory preparation must precede Builder materialization"
        );
        assert!(
            check < build,
            "syntax preflight must precede Builder execution"
        );
    }

    #[test]
    fn presentations_skill_exposes_one_fixed_existing_deck_editor_contract() {
        let workflows = include_str!("bundled/presentations/references/workflows.md");
        let editing = include_str!("bundled/presentations/references/editing-existing.md");
        let editor = include_str!("bundled/presentations/templates/editor.mjs");
        let capability: Value =
            serde_json::from_str(include_str!("bundled/presentations/office-capability.json"))
                .unwrap();

        for required in [
            "**Edit an existing `.pptx`:**",
            "ensure the selected script directory\nexists",
            "`templates/editor.mjs`",
            "`BEGIN EDIT REGION` / `END EDIT REGION`",
            "`node --check <editor>.mjs` as a separate `run_command` call",
            "exactly one static `--source` and one static `--output`",
            "`@mycopilot/presentation-sdk`",
            "The SDK writes a Host-only typed plan",
            "Whole-slide structure changes use the Editor's bounded slide operation",
            "at most once and last in the transaction",
            "Editor v1 has exactly one external-image route",
            "never repurpose an unrelated picture\nas a placeholder",
            "Adding a new external image and setting a slide image background are always\nunsupported",
            "Outside this exact `replaceImage` call",
            "do not copy Builder image syntax into the Editor",
            "Do not unzip or rewrite OOXML",
        ] {
            assert!(
                PRESENTATIONS_SOURCE.contains(required),
                "presentation instructions are missing `{required}`"
            );
        }

        for required in [
            "For every edit to an existing `.pptx`",
            "Create or reuse the dedicated script directory and every nested output parent",
            "before the baseline\nrender or Editor materialization",
            "## Clean up managed scripts",
            "node scripts/edit_deck.mjs --source source.pptx --output source-edited.pptx",
            "copy stable targets",
            "`/slide[3]/shape[@id=42]`",
            "must import only",
            "`editPresentation`, `input`, and `output` from `@mycopilot/presentation-sdk`",
            "`node --check scripts/edit_deck.mjs` in its own call",
            "Editor v1 supports only replacement of an existing inspected picture",
            "The `input()` string must exactly match a declared `inputs[].mountPath`",
            "Never repurpose an unrelated picture as\na placeholder",
            "Adding a new external image and setting a slide image background are always\nunsupported",
            "private placeholder to generic `deck.add` properties",
            "pre-publication\nfailure must leave the source and destination unchanged",
        ] {
            assert!(
                workflows.contains(required),
                "presentation workflow is missing `{required}`"
            );
        }

        for required in [
            "mkdir -p <parents...>",
            "Do not wait for `skills_materialize_resource` to fail",
            "delete the exact Editor and task-created\n   temporary files",
            "Never use recursive deletion",
            "import { editPresentation, input, output } from '@mycopilot/presentation-sdk'",
            "mode: 'saveAs'",
            "deck.set({ target, properties })",
            "deck.replaceText({ target, find, replace })",
            "deck.add({ parent, elementType, copyFrom?, position?, properties? })",
            "deck.remove({ target, properties? })",
            "deck.move({ target, newParent?, position?, properties? })",
            "deck.swap({ firstTarget, secondTarget })",
            "deck.replaceImage({ target, source: input(...) })",
            "The only supported external-image call in Editor v1",
            "This call cannot\ncreate a picture",
            "never\nrepurpose an unrelated picture as a placeholder",
            "always rejects adding an external image\nor setting a slide image background",
            "never copy\nBuilder image syntax into the Editor",
            "deck.updateTableCell({ target, text })",
            "deck.updateChart({ target, properties: { categories, series } })",
            "deck.addSlide({ layout?, title?, body?, backgroundColor? })",
            "deck.removeSlide({ slideNumber })",
            "deck.moveSlide({ slideNumber, newIndex })",
            "At most one whole-slide operation is\nallowed in a transaction",
            "discard every earlier element target and re-inspect",
            "Never recover from an unsupported or failed edit by",
            "render and read every final slide separately",
        ] {
            assert!(
                editing.contains(required),
                "existing-deck editing reference is missing `{required}`"
            );
        }

        let prepare_parents = editing
            .find("2. Choose one dedicated workspace-relative script directory")
            .unwrap();
        let render_baseline = editing
            .find("3. Render and read every slide whose layout or appearance may change")
            .unwrap();
        assert!(
            prepare_parents < render_baseline,
            "output parents must be prepared before the visual baseline render"
        );

        assert_eq!(editor.matches("// BEGIN EDIT REGION").count(), 1);
        assert_eq!(editor.matches("// END EDIT REGION").count(), 1);
        let begin = editor.find("// BEGIN EDIT REGION").unwrap();
        let end = editor.find("// END EDIT REGION").unwrap();
        assert!(begin < end, "the Editor edit region must be ordered");
        for required in [
            "import { editPresentation, input, output } from '@mycopilot/presentation-sdk'",
            "source: input(requiredValue('--source'))",
            "destination: output(requiredValue('--output'))",
            "mode: 'saveAs'",
            "deck.addSlide({",
            "deck.removeSlide({",
            "deck.moveSlide({",
            "deck.replaceText({",
            "deck.replaceImage({",
            "Replace only the exact inspected picture intended by the user; never repurpose another",
            "Adding external images/backgrounds is unsupported; do not guess file properties for deck.add",
            "deck.updateTableCell({",
            "deck.updateChart({",
            "deck.set({",
            "deck.add({",
            "deck.move({",
            "deck.swap({",
            "deck.remove({",
        ] {
            assert!(editor.contains(required), "Editor is missing `{required}`");
        }
        for forbidden in [
            "pptxgenjs",
            "node:fs",
            "child_process",
            "openPresentation",
            "findOne",
            "JSZip",
            "target: '/slide[7]'",
            "firstTarget: '/slide[6]'",
        ] {
            assert!(
                !editor.contains(forbidden),
                "Editor must not expose or import `{forbidden}`"
            );
        }

        assert_eq!(capability["contractVersion"], 11);
        let script = &capability["modes"]["script"];
        assert_eq!(script["builderTemplate"], "templates/builder.mjs");
        assert_eq!(script["editorTemplate"], "templates/editor.mjs");
        assert_eq!(script["editingReference"], "references/editing-existing.md");
        assert_eq!(script["routes"]["create"], "builder");
        assert_eq!(script["routes"]["editExisting"], "editor");
        assert_eq!(script["workspaceParents"]["mustExistBeforeUse"], true);
        assert_eq!(
            script["workspaceParents"]["appliesTo"],
            serde_json::json!([
                "materializationDestination",
                "managedOutput",
                "renderOutput"
            ])
        );
        assert_eq!(
            script["workspaceParents"]["hostPreflightBeforeScript"],
            true
        );
        assert_eq!(
            script["workspaceParents"]["hostPreflightBeforeRenderer"],
            true
        );
        let editor_contract = &script["editor"];
        assert_eq!(editor_contract["sourceArgument"], "--source");
        assert_eq!(editor_contract["outputArgument"], "--output");
        assert_eq!(editor_contract["defaultPublishMode"], "saveAs");
        assert_eq!(
            editor_contract["supportedPublishModes"],
            serde_json::json!(["saveAs"])
        );
        assert_eq!(editor_contract["inPlace"], false);
        assert_eq!(editor_contract["positionShape"], "taggedIndexAfterOrBefore");
        assert_eq!(
            editor_contract["structuralScope"],
            "elementsAndOneTerminalWholeSlideMutation"
        );
        assert_eq!(
            editor_contract["wholeSlideMutations"],
            "editorAtMostOneAndLastThenReinspect"
        );
        assert_eq!(
            editor_contract["targetRules"]["elementTarget"],
            "inspectedIdRequired"
        );
        assert_eq!(
            editor_contract["targetRules"]["parent"],
            "slideContainerAllowed"
        );
        assert_eq!(
            editor_contract["externalImage"]["supportedOperation"],
            "replaceImage"
        );
        assert_eq!(
            editor_contract["externalImage"]["target"],
            "existingInspectedPictureRequired"
        );
        assert_eq!(
            editor_contract["externalImage"]["source"],
            "inputMatchingDeclaredMountPath"
        );
        assert_eq!(
            editor_contract["externalImage"]["genericAddFileInput"],
            false
        );
        assert_eq!(
            editor_contract["externalImage"]["addNewImage"],
            "unsupported"
        );
        assert_eq!(
            editor_contract["externalImage"]["setImageBackground"],
            "unsupported"
        );
        assert_eq!(editor_contract["facade"]["apiVersion"], 1);
        assert_eq!(
            editor_contract["facade"]["specifier"],
            "@mycopilot/presentation-sdk"
        );
        assert_eq!(
            editor_contract["facade"]["operations"],
            serde_json::json!([
                "editPresentation",
                "input",
                "output",
                "addSlide",
                "removeSlide",
                "moveSlide",
                "set",
                "replaceText",
                "add",
                "remove",
                "move",
                "swap",
                "replaceImage",
                "updateTableCell",
                "updateChart"
            ])
        );
        assert_eq!(editor_contract["transaction"]["publication"], "atomic");
        assert_eq!(
            editor_contract["transaction"]["failureBeforePublish"],
            "sourceAndDestinationUnchanged"
        );

        let service = SkillsService::new().with_bundled_source().unwrap();
        let descriptor = service
            .list()
            .unwrap()
            .skills()
            .iter()
            .find(|skill| skill.id().local_id() == PRESENTATIONS_LOCAL_ID)
            .unwrap()
            .clone();
        let activated = service.activate(&[descriptor.selection()]).unwrap();
        let resources = service.resource_session(&activated).unwrap();
        let package = resources.package_uris().into_iter().next().unwrap();
        let editor_uri =
            package.resource(SkillResourcePath::parse("templates/editor.mjs").unwrap());
        let materialized_source = resources
            .read_text(&editor_uri, SkillResourceTextReadOptions::default())
            .unwrap();
        assert_eq!(materialized_source.text(), editor);
        let workspace = tempdir().unwrap();
        fs::create_dir(workspace.path().join("scripts")).unwrap();
        let request = SkillMaterializationRequest::new(
            editor_uri,
            workspace.path(),
            SkillMaterializationDestination::parse("scripts/edit_deck.mjs").unwrap(),
        )
        .unwrap();
        let outcome = SkillResourceMaterializer::new()
            .materialize(&resources, &request)
            .unwrap();
        assert_eq!(outcome.status(), SkillMaterializationStatus::Created);
        assert_eq!(
            fs::read_to_string(workspace.path().join("scripts/edit_deck.mjs")).unwrap(),
            editor
        );
    }

    #[test]
    fn presentations_skill_requires_one_visual_verdict_per_inspected_slide() {
        let workflows = include_str!("bundled/presentations/references/workflows.md");

        for required in [
            "record its authoritative slide count `N`",
            "contact sheet is optional and is overview-only",
            "For every slide `1..N`, make a separate `render` call",
            "that `pageOrSlide` and a unique `outputPath`",
            "Any later deck edit invalidates the ledger",
            "Without exactly `N` successful numbered verdicts",
            "It does not prove slide content, visual quality, or successful per-slide inspection",
        ] {
            assert!(
                PRESENTATIONS_SOURCE.contains(required),
                "presentation instructions are missing `{required}`"
            );
        }

        for required in [
            "Repeat\nthis inspection after the last edit because earlier counts become stale",
            "overview only. It cannot prove that every slide is present, fully visible, or readable",
            "\"pageOrSlide\": 1",
            "give every call a unique `outputPath`",
            "Exactly `N` numbered verdicts are required",
            "`total` and `pageSelection` describe source or requested selection metadata",
            "`layoutCoverage.evidence = trustedRendererGeometry` proves only",
            "An output filename such as `complete`, `all`, or `full` is never coverage evidence",
            "Do not ask a reviewer to infer the slide count from a dense",
        ] {
            assert!(
                workflows.contains(required),
                "presentation workflow is missing `{required}`"
            );
        }

        for stale in [
            "Render all changed slides in one contact sheet",
            "Render every slide to an explicit contact-sheet image",
            "Run one native `render` request covering every final slide",
            "render every slide in separate retry calls",
        ] {
            assert!(
                !PRESENTATIONS_SOURCE.contains(stale) && !workflows.contains(stale),
                "presentation instructions retain stale contact-sheet rule `{stale}`"
            );
        }

        let capability: Value =
            serde_json::from_str(include_str!("bundled/presentations/office-capability.json"))
                .unwrap();
        assert_eq!(capability["contractVersion"], 11);
        let render_output = &capability["modes"]["native"]["renderOutput"];
        assert_eq!(
            render_output["contactSheetSemantics"],
            "overview_only_not_coverage_evidence"
        );
        assert_eq!(render_output["layoutCoverageField"], "layoutCoverage");
        assert_eq!(
            render_output["layoutCoverageSemantics"],
            "trusted_renderer_geometry_only_not_visual_content_evidence"
        );
        let verification = &render_output["visualVerification"];
        assert_eq!(verification["slideCountSource"], "inspect");
        assert_eq!(verification["requestMode"], "one_slide_per_render");
        assert_eq!(verification["requestField"], "pageOrSlide");
        assert_eq!(verification["uniqueOutputPathPerSlide"], true);
        assert_eq!(verification["consumeEveryReturnedReadPath"], true);
        assert_eq!(verification["requiredVerdicts"], "one_per_inspected_slide");
        assert_eq!(verification["ledgerInvalidation"], "any_deck_edit");
        assert_eq!(
            verification["coverageCannotBeInferredFrom"],
            serde_json::json!([
                "total",
                "pageSelection",
                "layoutCoverage",
                "outputFilename",
                "contactSheet"
            ])
        );
    }

    #[test]
    fn bundled_resources_are_listable_readable_and_builder_templates_materialize() {
        let service = SkillsService::new().with_bundled_source().unwrap();
        for (local_id, heading, template_path, marker, destination) in [
            (
                DOCUMENTS_LOCAL_ID,
                "# Word document workflows",
                "templates/builder.py",
                "from docx import Document",
                "build_document.py",
            ),
            (
                PRESENTATIONS_LOCAL_ID,
                "# Presentation workflows",
                "templates/builder.mjs",
                "pptxgenjs",
                "build_presentation.mjs",
            ),
            (
                SPREADSHEETS_LOCAL_ID,
                "# Spreadsheet workflows",
                "templates/builder.py",
                "openpyxl",
                "build_spreadsheet.py",
            ),
        ] {
            let descriptor = service
                .list()
                .unwrap()
                .skills()
                .iter()
                .find(|skill| skill.id().local_id() == local_id)
                .unwrap()
                .clone();
            let activated = service.activate(&[descriptor.selection()]).unwrap();
            let resources = service.resource_session(&activated).unwrap();
            let package = resources.package_uris().into_iter().next().unwrap();
            let listed = resources
                .list(&package, &SkillResourceListOptions::default())
                .unwrap();
            assert_eq!(listed.entries().len(), 5);
            assert!(listed
                .entries()
                .iter()
                .any(|entry| entry.descriptor().path() == template_path));
            let workflow =
                package.resource(SkillResourcePath::parse("references/workflows.md").unwrap());
            let template = package.resource(SkillResourcePath::parse(template_path).unwrap());

            let page = resources
                .read_text(&workflow, SkillResourceTextReadOptions::default())
                .unwrap();
            let builder = resources
                .read_text(&template, SkillResourceTextReadOptions::default())
                .unwrap();

            assert!(page.text().contains(heading));
            assert!(page.text().contains("Managed Builder"));
            if local_id == DOCUMENTS_LOCAL_ID {
                assert_eq!(
                    page.text()
                        .matches("\"runtimeProfile\": \"documents\"")
                        .count(),
                    1
                );
                assert!(page.text().contains("Omit `runtimeProfile`"));
                assert!(page.text().contains("`observe`: the Host"));
            } else {
                assert!(!page.text().contains("\"runtimeProfile\":"));
            }
            assert!(page.text().contains("--output"));
            assert!(!page.text().contains("runtime.requiredPackages"));
            assert!(!page.text().contains("\"provider\": \"managedArtifact\""));
            assert!(page.text().contains("\"inputs\":"));
            assert!(page.text().contains("MYCOPILOT_INPUT_ROOT"));
            if local_id == SPREADSHEETS_LOCAL_ID {
                assert!(page.text().contains("Omit `runtimeProfile` and `observe`"));
                assert!(page.text().contains("artifactObservation"));
                assert!(page.text().contains("failure, timeout, or cancellation"));
            } else {
                assert!(page.text().contains("observe.expectedOutputs"));
                assert!(page.text().contains("observe.additionalRoots"));
                assert!(page.text().contains("artifactObservation.expectedOutputs"));
                assert!(page.text().contains("non-zero exit"));
            }
            assert!(!page.truncated());
            assert!(builder.text().contains(marker));
            assert!(builder.text().contains("MYCOPILOT_INPUT_ROOT"));
            assert!(!builder.truncated());

            let workspace = tempdir().unwrap();
            let request = SkillMaterializationRequest::new(
                template,
                workspace.path(),
                SkillMaterializationDestination::parse(destination).unwrap(),
            )
            .unwrap();
            let outcome = SkillResourceMaterializer::new()
                .materialize(&resources, &request)
                .unwrap();
            assert_eq!(outcome.status(), SkillMaterializationStatus::Created);
            assert_eq!(
                fs::read_to_string(workspace.path().join(destination)).unwrap(),
                builder.text()
            );
        }
    }

    #[test]
    fn embedded_source_rejects_stale_and_unknown_selections() {
        let source = BundledSkillSource::new().unwrap();
        let descriptor = source
            .list()
            .unwrap()
            .skills()
            .iter()
            .find(|skill| skill.id().local_id() == DOCUMENTS_LOCAL_ID)
            .unwrap()
            .clone();
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
