use mycopilot_core::skills::{SkillProvenance, SkillResolveRequest, SkillScope, SkillsService};
use std::path::Path;

#[test]
fn public_core_api_resolves_a_catalog_selection_without_exposing_source_paths() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace");
    let service = SkillsService::new();
    let catalog = service
        .list_workspace("fixture-workspace", &workspace)
        .unwrap();
    let descriptor = catalog.skills.first().unwrap();
    let request = SkillResolveRequest {
        skill_id: descriptor.id.clone(),
        expected_revision: descriptor.revision.clone(),
    };

    let resolved = service
        .resolve_workspace_skill("fixture-workspace", &workspace, &request)
        .unwrap();

    assert_eq!(resolved.id(), descriptor.id);
    assert_eq!(resolved.scope(), SkillScope::Workspace);
    assert_eq!(resolved.revision(), descriptor.revision);
    match resolved.provenance() {
        SkillProvenance::Workspace { relative_path, .. } => {
            assert_eq!(relative_path, &descriptor.relative_path);
        }
        _ => panic!("fixture must resolve from the workspace source"),
    }
    assert!(resolved.instructions().contains("SKILL_FIXTURE_V1"));
    assert!(resolved.source_text().starts_with("---\n"));
    assert!(!format!("{resolved:?}").contains("SKILL_FIXTURE_V1"));
}
