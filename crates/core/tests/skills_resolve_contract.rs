use mycopilot_core::skills::{
    SkillActivationScope, SkillProvenance, SkillSourceKind, SkillTrust, SkillsService,
    SKILL_PACKAGE_FORMAT_VERSION,
};
use std::path::Path;

#[test]
fn public_core_api_resolves_a_catalog_selection_without_exposing_source_paths() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace");
    let service = SkillsService::for_workspace("fixture-workspace", &workspace).unwrap();
    let catalog = service.list().unwrap();
    let descriptor = catalog.skills().first().unwrap();

    let resolved = service.resolve(&descriptor.selection()).unwrap();

    assert_eq!(resolved.descriptor(), descriptor);
    assert_eq!(resolved.id(), descriptor.id());
    assert_eq!(resolved.source_kind(), SkillSourceKind::Workspace);
    assert_eq!(resolved.trust(), SkillTrust::Untrusted);
    assert_eq!(resolved.activation_scope(), SkillActivationScope::Run);
    assert_eq!(resolved.revision(), descriptor.revision());
    assert_eq!(resolved.format_version(), SKILL_PACKAGE_FORMAT_VERSION);
    assert!(resolved.resources().is_empty());
    match resolved.provenance() {
        SkillProvenance::Workspace { relative_path, .. } => {
            assert_eq!(
                relative_path,
                ".agents/skills/repository-evidence-auditor/SKILL.md"
            );
        }
        _ => panic!("fixture must resolve from the workspace source"),
    }
    assert!(resolved.instructions().contains("SKILL_FIXTURE_V1"));
    assert!(resolved.source_text().starts_with("---\n"));
    assert!(!format!("{resolved:?}").contains("SKILL_FIXTURE_V1"));
}

#[test]
fn every_public_catalog_descriptor_is_a_valid_resolve_selection() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace");
    let service = SkillsService::for_workspace("fixture-workspace", workspace).unwrap();
    let catalog = service.list().unwrap();

    for descriptor in catalog.skills() {
        let resolved = service.resolve(&descriptor.selection()).unwrap();
        assert_eq!(resolved.descriptor(), descriptor);
    }
}
