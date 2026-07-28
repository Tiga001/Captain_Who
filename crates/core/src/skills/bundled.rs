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
pub const PRESENTATIONS_LOCAL_ID: &str = "presentations";
pub const SPREADSHEETS_LOCAL_ID: &str = "spreadsheets";

const DOCUMENTS_PATH: &str = "documents/SKILL.md";
const DOCUMENTS_SOURCE: &str = include_str!("bundled/documents/SKILL.md");
const DOCUMENTS_RESOURCES: &[EmbeddedSkillResource] = &[
    EmbeddedSkillResource {
        path: "office-capability.json",
        bytes: include_bytes!("bundled/documents/office-capability.json"),
    },
    EmbeddedSkillResource {
        path: "references/workflows.md",
        bytes: include_bytes!("bundled/documents/references/workflows.md"),
    },
    EmbeddedSkillResource {
        path: "templates/builder.py",
        bytes: include_bytes!("bundled/documents/templates/builder.py"),
    },
];

const IMAGE_GENERATION_PATH: &str = "image-generation/SKILL.md";
const IMAGE_GENERATION_SOURCE: &str = include_str!("bundled/image-generation/SKILL.md");

const PRESENTATIONS_PATH: &str = "presentations/SKILL.md";
const PRESENTATIONS_SOURCE: &str = include_str!("bundled/presentations/SKILL.md");
const PRESENTATIONS_RESOURCES: &[EmbeddedSkillResource] = &[
    EmbeddedSkillResource {
        path: "office-capability.json",
        bytes: include_bytes!("bundled/presentations/office-capability.json"),
    },
    EmbeddedSkillResource {
        path: "references/workflows.md",
        bytes: include_bytes!("bundled/presentations/references/workflows.md"),
    },
    EmbeddedSkillResource {
        path: "templates/builder.mjs",
        bytes: include_bytes!("bundled/presentations/templates/builder.mjs"),
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
        path: "references/workflows.md",
        bytes: include_bytes!("bundled/spreadsheets/references/workflows.md"),
    },
    EmbeddedSkillResource {
        path: "templates/builder.py",
        bytes: include_bytes!("bundled/spreadsheets/templates/builder.py"),
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
        local_id: PRESENTATIONS_LOCAL_ID,
        relative_path: PRESENTATIONS_PATH,
        source_text: PRESENTATIONS_SOURCE,
        resources: PRESENTATIONS_RESOURCES,
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
        SkillErrorCode, SkillResourceKind, SkillRevision, SKILL_PACKAGE_FORMAT_VERSION_V3,
    };
    use crate::skills::{
        SkillMaterializationDestination, SkillMaterializationRequest, SkillMaterializationStatus,
        SkillResourceListOptions, SkillResourceMaterializer, SkillResourcePath,
        SkillResourceTextReadOptions, SkillsService,
    };
    use serde_json::Value;
    use std::fs;
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
        assert_eq!(catalog.skills().len(), 4);
        assert_eq!(
            catalog
                .skills()
                .iter()
                .map(|skill| skill.id().as_str())
                .collect::<Vec<_>>(),
            vec![
                "bundled:application:documents",
                "bundled:application:image-generation",
                "bundled:application:presentations",
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
        assert!(package
            .instructions()
            .contains("Use one of two supported paths"));
        assert!(!package.instructions().contains("description:"));
        assert_eq!(package.revision(), descriptor.revision());
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
            ),
            (
                SPREADSHEETS_LOCAL_ID,
                "office_spreadsheet",
                ".xlsx",
                "spreadsheets",
                "templates/builder.py",
                "python",
                "openpyxl",
                "3.1.5",
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
            assert_eq!(package.resources().len(), 3);
            assert_eq!(
                package.resources().entries()[0].path(),
                "office-capability.json"
            );
            assert_eq!(
                package.resources().entries()[0].kind(),
                SkillResourceKind::Other
            );
            assert_eq!(
                package.resources().entries()[1].path(),
                "references/workflows.md"
            );
            assert_eq!(
                package.resources().entries()[1].kind(),
                SkillResourceKind::Reference
            );
            assert_eq!(package.resources().entries()[2].path(), template_path);
            assert_eq!(
                package.resources().entries()[2].kind(),
                SkillResourceKind::Other
            );
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
            assert_eq!(capability["contractVersion"], 7);
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
            assert_eq!(render_output["sourceField"], "source");
            assert_eq!(render_output["consumerTool"], "read_image");
            assert_eq!(render_output["consumerField"], "source");
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
                "host_from_materialized_builder_receipt"
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
            assert_eq!(script["inputs"]["sourceField"], "source");
            assert_eq!(script["inputs"]["rootEnvironment"], "MYCOPILOT_INPUT_ROOT");
            assert_eq!(
                script["inputs"]["sourceTypes"],
                serde_json::json!([
                    "attachment",
                    "workspace",
                    "external",
                    "generated_artifact",
                    "skill_resource"
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
            assert_eq!(listed.entries().len(), 3);
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
            assert!(!page.text().contains("\"runtimeProfile\":"));
            assert!(page.text().contains("--output"));
            assert!(!page.text().contains("runtime.requiredPackages"));
            assert!(!page.text().contains("\"provider\": \"managedArtifact\""));
            assert!(page.text().contains("\"inputs\":"));
            assert!(page.text().contains("MYCOPILOT_INPUT_ROOT"));
            assert!(page.text().contains("observe.expectedOutputs"));
            assert!(page.text().contains("observe.additionalRoots"));
            assert!(page.text().contains("artifactObservation.expectedOutputs"));
            assert!(page.text().contains("non-zero exit"));
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
