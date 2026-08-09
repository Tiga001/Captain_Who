use super::*;
use serde_json::json;

fn visible_section(id: WorldStateSectionId, value: &str) -> WorldStateSectionEnvelope {
    WorldStateSectionEnvelope::model_visible(
        id,
        WorldStateLifetime::Conversation,
        json!({
            "authority": {
                "requestId": "request-random-123",
                "updatedAt": 1_754_000_000,
                "sha256": "abcdef",
                "secret": "do-not-render"
            },
            "value": value
        }),
        json!({ "value": value }),
    )
    .unwrap()
}

fn host_section(id: WorldStateSectionId, value: &str) -> WorldStateSectionEnvelope {
    WorldStateSectionEnvelope::host_only(
        id,
        WorldStateLifetime::Conversation,
        json!({
            "apiKey": "secret-key",
            "requestId": "request-random-456",
            "value": value
        }),
    )
    .unwrap()
}

#[test]
fn canonical_domain_sections_keep_authority_projection_and_lifetime_separate() {
    let permissions = crate::protocol::AgentPermissions {
        read: crate::protocol::AgentReadPermission::All,
        write: crate::protocol::AgentWritePermission::WorkspaceOnly,
        command: crate::protocol::AgentCommandPermission::AutoApprove,
        command_safety: crate::protocol::AgentCommandSafetyPolicy::Guarded,
        patch: crate::protocol::AgentPatchPermission::RequireApproval,
    };
    let conversation_permissions =
        effective_permissions_section(permissions, WorldStateLifetime::Conversation).unwrap();
    let run_permissions =
        effective_permissions_section(permissions, WorldStateLifetime::Run).unwrap();
    assert_eq!(
        conversation_permissions.model_projection,
        run_permissions.model_projection
    );
    assert_eq!(
        conversation_permissions.id,
        WorldStateSectionId::EffectivePermissions
    );
    assert_ne!(
        conversation_permissions.revision, run_permissions.revision,
        "lifetime remains part of the authoritative section identity"
    );

    let workspace = crate::protocol::AgentWorkspaceContext {
        project_id: Some("project-secret".to_string()),
        display_name: Some("Visible workspace".to_string()),
        root_path: Some("/private/authoritative/root".to_string()),
    };
    let workspace =
        workspace_binding_section(Some(&workspace), WorldStateLifetime::Conversation).unwrap();
    assert_eq!(workspace.state["rootPath"], "/private/authoritative/root");
    let rendered_projection = serde_json::to_string(&workspace.model_projection).unwrap();
    assert!(rendered_projection.contains("Visible workspace"));
    assert!(rendered_projection.contains("workspace_relative"));
    assert!(!rendered_projection.contains("/private/authoritative/root"));
    assert!(!rendered_projection.contains("project-secret"));

    let capabilities = model_capabilities_section(
        crate::protocol::ModelCapabilities { image_input: true },
        WorldStateLifetime::Run,
    )
    .unwrap();
    assert_eq!(capabilities.visibility, WorldStateVisibility::HostOnly);
    assert!(capabilities.model_projection.is_none());

    let selection = model_selection_section(
        "provider/model-v1",
        crate::protocol::ModelCapabilities { image_input: true },
        WorldStateLifetime::Conversation,
    )
    .unwrap();
    assert_eq!(selection.visibility, WorldStateVisibility::ModelVisible);
    assert_eq!(
        selection.model_projection.as_ref().unwrap(),
        &serde_json::json!({
            "configuredModelId": "provider/model-v1",
            "capabilities": {
                "imageInput": true,
            }
        })
    );
}

#[test]
fn model_selection_capability_changes_generate_visible_replacements() {
    let text_only = WorldStateSnapshot::new(
        "model-epoch",
        0,
        vec![model_selection_section(
            "provider/model-v1",
            crate::protocol::ModelCapabilities { image_input: false },
            WorldStateLifetime::Conversation,
        )
        .unwrap()],
    )
    .unwrap();
    let vision = WorldStateSnapshot::new(
        "model-epoch",
        1,
        vec![model_selection_section(
            "provider/model-v1",
            crate::protocol::ModelCapabilities { image_input: true },
            WorldStateLifetime::Conversation,
        )
        .unwrap()],
    )
    .unwrap();

    let upgrade = WorldStateDiff::between(&text_only, &vision).unwrap();
    let upgrade_projection = upgrade
        .model_projection_against(&text_only, WorldStateLifetime::Conversation)
        .unwrap()
        .unwrap()
        .render_sanitized_text();
    assert!(upgrade_projection.contains("\"op\":\"replace\""));
    assert!(upgrade_projection.contains("\"imageInput\":true"));

    let text_only_again = WorldStateSnapshot::new(
        "model-epoch",
        2,
        vec![model_selection_section(
            "provider/model-v1",
            crate::protocol::ModelCapabilities { image_input: false },
            WorldStateLifetime::Conversation,
        )
        .unwrap()],
    )
    .unwrap();
    let downgrade = WorldStateDiff::between(&vision, &text_only_again).unwrap();
    let downgrade_projection = downgrade
        .model_projection_against(&vision, WorldStateLifetime::Conversation)
        .unwrap()
        .unwrap()
        .render_sanitized_text();
    assert!(downgrade_projection.contains("\"op\":\"replace\""));
    assert!(downgrade_projection.contains("\"imageInput\":false"));
}

#[test]
fn snapshot_order_and_sha256_revision_are_deterministic() {
    let workspace = visible_section(WorldStateSectionId::WorkspaceBinding, "workspace");
    let permissions = visible_section(WorldStateSectionId::EffectivePermissions, "permissions");

    let left = WorldStateSnapshot::new("epoch-a", 0, vec![workspace.clone(), permissions.clone()])
        .unwrap();
    let right = WorldStateSnapshot::new("epoch-b", 42, vec![permissions, workspace]).unwrap();

    assert_eq!(
        left.sections
            .iter()
            .map(|section| section.id.as_str())
            .collect::<Vec<_>>(),
        vec!["permissions.effective", "workspace.binding"]
    );
    assert_eq!(left.revision, right.revision);
    assert!(left.revision.starts_with(WORLD_STATE_REVISION_PREFIX));
    assert_eq!(left.revision.len(), WORLD_STATE_REVISION_PREFIX.len() + 64);

    let serialized = serde_json::to_string(&left).unwrap();
    let restored: WorldStateSnapshot = serde_json::from_str(&serialized).unwrap();
    restored.validate().unwrap();
    assert_eq!(restored, left);
    assert_eq!(restored.canonical_json(), left.canonical_json());
}

#[test]
fn canonical_hash_ignores_json_object_insertion_order() {
    let first: Value = serde_json::from_str(r#"{"b":2,"a":{"z":3,"y":4}}"#).unwrap();
    let second: Value = serde_json::from_str(r#"{"a":{"y":4,"z":3},"b":2}"#).unwrap();
    let left = WorldStateSectionEnvelope::model_visible(
        WorldStateSectionId::InteractionProfile,
        WorldStateLifetime::Conversation,
        first.clone(),
        first,
    )
    .unwrap();
    let right = WorldStateSectionEnvelope::model_visible(
        WorldStateSectionId::InteractionProfile,
        WorldStateLifetime::Conversation,
        second.clone(),
        second,
    )
    .unwrap();

    assert_eq!(left.revision, right.revision);
}

#[test]
fn reducer_supports_replace_remove_and_later_re_add() {
    let initial = WorldStateSnapshot::new(
        "epoch",
        0,
        vec![
            visible_section(WorldStateSectionId::EffectivePermissions, "ask"),
            visible_section(WorldStateSectionId::WorkspaceBinding, "workspace-a"),
        ],
    )
    .unwrap();
    let target_one = WorldStateSnapshot::new(
        "epoch",
        1,
        vec![visible_section(
            WorldStateSectionId::EffectivePermissions,
            "allow",
        )],
    )
    .unwrap();
    let diff_one = WorldStateDiff::between(&initial, &target_one).unwrap();

    assert!(matches!(
        diff_one.operations.as_slice(),
        [
            WorldStateOperation::Replace { .. },
            WorldStateOperation::Remove { .. }
        ]
    ));

    let target_two = WorldStateSnapshot::new(
        "epoch",
        2,
        vec![
            visible_section(WorldStateSectionId::EffectivePermissions, "allow"),
            visible_section(WorldStateSectionId::WorkspaceBinding, "workspace-b"),
        ],
    )
    .unwrap();
    let diff_two = WorldStateDiff::between(&target_one, &target_two).unwrap();
    assert!(matches!(
        diff_two.operations.as_slice(),
        [WorldStateOperation::Add { .. }]
    ));

    let folded = WorldStateReducer::fold(initial, &[diff_one, diff_two]).unwrap();
    assert_eq!(folded, target_two);
}

#[test]
fn reducer_rejects_invalid_base_and_sequence() {
    let initial = WorldStateSnapshot::new(
        "epoch",
        0,
        vec![visible_section(
            WorldStateSectionId::EffectivePermissions,
            "ask",
        )],
    )
    .unwrap();
    let target = WorldStateSnapshot::new(
        "epoch",
        1,
        vec![visible_section(
            WorldStateSectionId::EffectivePermissions,
            "allow",
        )],
    )
    .unwrap();
    let diff = WorldStateDiff::between(&initial, &target).unwrap();

    let mut wrong_base = diff.clone();
    wrong_base.base_revision = format!("{WORLD_STATE_REVISION_PREFIX}{}", "0".repeat(64));
    assert!(matches!(
        WorldStateReducer::fold(initial.clone(), &[wrong_base]),
        Err(WorldStateError::BaseRevisionMismatch { .. })
    ));

    let mut wrong_sequence = diff;
    wrong_sequence.sequence = 7;
    assert!(matches!(
        WorldStateReducer::fold(initial, &[wrong_sequence]),
        Err(WorldStateError::SequenceMismatch {
            expected: 1,
            actual: 7
        })
    ));
}

#[test]
fn reducer_validates_remove_tombstone_and_replace_precondition() {
    let initial = WorldStateSnapshot::new(
        "epoch",
        0,
        vec![visible_section(
            WorldStateSectionId::EffectivePermissions,
            "ask",
        )],
    )
    .unwrap();
    let removed = WorldStateSnapshot::new("epoch", 1, vec![]).unwrap();
    let mut removal = WorldStateDiff::between(&initial, &removed).unwrap();
    let WorldStateOperation::Remove { tombstone } = &mut removal.operations[0] else {
        panic!("expected remove operation");
    };
    tombstone.removed_revision = format!("{WORLD_STATE_REVISION_PREFIX}{}", "0".repeat(64));
    assert!(matches!(
        WorldStateReducer::fold(initial.clone(), &[removal]),
        Err(WorldStateError::SectionPreconditionMismatch { .. })
    ));

    let target = WorldStateSnapshot::new(
        "epoch",
        1,
        vec![visible_section(
            WorldStateSectionId::EffectivePermissions,
            "allow",
        )],
    )
    .unwrap();
    let mut replacement = WorldStateDiff::between(&initial, &target).unwrap();
    let WorldStateOperation::Replace { previous, .. } = &mut replacement.operations[0] else {
        panic!("expected replace operation");
    };
    previous.visibility = WorldStateVisibility::HostOnly;
    assert!(matches!(
        WorldStateReducer::fold(initial, &[replacement]),
        Err(WorldStateError::SectionPreconditionMismatch { .. })
    ));
}

#[test]
fn host_only_state_and_journal_metadata_never_enter_model_text() {
    let snapshot = WorldStateSnapshot::new(
        "epoch-random-secret",
        99,
        vec![
            visible_section(WorldStateSectionId::WorkspaceBinding, "available"),
            host_section(WorldStateSectionId::ModelCapabilities, "image-input"),
        ],
    )
    .unwrap();
    let text = snapshot
        .model_projection(WorldStateLifetime::Conversation)
        .unwrap()
        .render_sanitized_text();

    assert!(text.contains("workspace.binding"));
    assert!(text.contains("available"));
    assert!(text.contains("\"lifetime\":\"conversation\""));
    assert!(text.contains("full record replaces only its declared lifetime"));
    assert!(!text.contains("model.capabilities"));
    assert!(!text.contains("secret-key"));
    assert!(!text.contains("request-random"));
    assert!(!text.contains("updatedAt"));
    assert!(!text.contains("sha256"));
    assert!(!text.contains("epoch-random-secret"));
    assert!(!text.contains(WORLD_STATE_REVISION_PREFIX));
    assert!(!text.contains("\"sequence\""));
}

#[test]
fn host_only_diff_produces_no_model_record() {
    let initial = WorldStateSnapshot::new(
        "epoch",
        0,
        vec![host_section(
            WorldStateSectionId::ModelCapabilities,
            "disabled",
        )],
    )
    .unwrap();
    let target = WorldStateSnapshot::new(
        "epoch",
        1,
        vec![host_section(
            WorldStateSectionId::ModelCapabilities,
            "enabled",
        )],
    )
    .unwrap();
    let diff = WorldStateDiff::between(&initial, &target).unwrap();

    assert_eq!(
        diff.model_projection_against(&initial, WorldStateLifetime::Conversation)
            .unwrap(),
        None
    );
    assert_ne!(initial.revision, target.revision);
    assert_eq!(
        initial
            .model_projection_revision(WorldStateLifetime::Conversation)
            .unwrap(),
        target
            .model_projection_revision(WorldStateLifetime::Conversation)
            .unwrap()
    );
}

#[test]
fn model_diff_handles_visibility_transitions_without_host_data() {
    let visible = visible_section(WorldStateSectionId::WorkspaceBinding, "available");
    let initial = WorldStateSnapshot::new("epoch", 0, vec![visible]).unwrap();
    let target = WorldStateSnapshot::new(
        "epoch",
        1,
        vec![host_section(
            WorldStateSectionId::WorkspaceBinding,
            "secret-host-state",
        )],
    )
    .unwrap();
    let diff = WorldStateDiff::between(&initial, &target).unwrap();
    let record = diff
        .model_projection_against(&initial, WorldStateLifetime::Conversation)
        .unwrap()
        .expect("visible-to-host-only must remove the visible section");
    let text = record.render_sanitized_text();

    assert!(text.contains("\"lifetime\":\"conversation\""));
    assert!(text.contains("\"op\":\"remove\""));
    assert!(text.contains("workspace.binding"));
    assert!(!text.contains("secret-host-state"));
}

#[test]
fn model_projection_requires_one_explicit_lifetime_but_accepts_empty_snapshots() {
    let empty = WorldStateSnapshot::new("empty-run-epoch", 0, vec![]).unwrap();
    let projection = empty
        .model_projection(WorldStateLifetime::Run)
        .expect("an empty snapshot can establish an explicit Run ledger");
    let value = serde_json::to_value(&projection).unwrap();
    assert_eq!(value["recordType"], "full");
    assert_eq!(value["lifetime"], "run");
    assert_eq!(value["sections"], json!([]));

    let mixed = WorldStateSnapshot::new(
        "mixed-epoch",
        0,
        vec![
            visible_section(WorldStateSectionId::WorkspaceBinding, "workspace"),
            WorldStateSectionEnvelope::host_only(
                WorldStateSectionId::ModelCapabilities,
                WorldStateLifetime::Run,
                json!({"imageInput": true}),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    assert!(matches!(
        mixed.model_projection(WorldStateLifetime::Conversation),
        Err(WorldStateError::SectionLifetimeMismatch {
            section_id: WorldStateSectionId::ModelCapabilities,
            expected: WorldStateLifetime::Conversation,
            actual: WorldStateLifetime::Run,
        })
    ));
}

#[test]
fn model_diff_rejects_a_result_that_crosses_lifetime_ledgers() {
    let initial = WorldStateSnapshot::new(
        "epoch",
        0,
        vec![visible_section(
            WorldStateSectionId::EffectivePermissions,
            "ask",
        )],
    )
    .unwrap();
    let target = WorldStateSnapshot::new(
        "epoch",
        1,
        vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::EffectivePermissions,
            WorldStateLifetime::Run,
            json!({"value": "allow"}),
            json!({"value": "allow"}),
        )
        .unwrap()],
    )
    .unwrap();
    let diff = WorldStateDiff::between(&initial, &target).unwrap();

    assert!(matches!(
        diff.model_projection_against(&initial, WorldStateLifetime::Conversation),
        Err(WorldStateError::SectionLifetimeMismatch {
            section_id: WorldStateSectionId::EffectivePermissions,
            expected: WorldStateLifetime::Conversation,
            actual: WorldStateLifetime::Run,
        })
    ));
}

#[test]
fn full_plus_diff_fold_is_exactly_equivalent_to_target_full() {
    let full = WorldStateSnapshot::new(
        "epoch",
        0,
        vec![
            visible_section(WorldStateSectionId::EffectivePermissions, "ask"),
            host_section(WorldStateSectionId::ModelCapabilities, "disabled"),
        ],
    )
    .unwrap();
    let state_one = WorldStateSnapshot::new(
        "epoch",
        1,
        vec![
            visible_section(WorldStateSectionId::EffectivePermissions, "allow"),
            host_section(WorldStateSectionId::ModelCapabilities, "disabled"),
            visible_section(WorldStateSectionId::WorkspaceBinding, "workspace"),
        ],
    )
    .unwrap();
    let state_two = WorldStateSnapshot::new(
        "epoch",
        2,
        vec![
            visible_section(WorldStateSectionId::EffectivePermissions, "allow"),
            host_section(WorldStateSectionId::ModelCapabilities, "enabled"),
        ],
    )
    .unwrap();
    let diff_one = WorldStateDiff::between(&full, &state_one).unwrap();
    let diff_two = WorldStateDiff::between(&state_one, &state_two).unwrap();

    let folded = WorldStateReducer::fold(full, &[diff_one, diff_two]).unwrap();
    assert_eq!(folded, state_two);

    let rebased = folded.rebase("new-epoch").unwrap();
    assert_eq!(rebased.sequence, 0);
    assert_eq!(rebased.revision, folded.revision);
    assert_eq!(rebased.sections, folded.sections);
}

#[test]
fn empty_or_duplicate_diffs_are_rejected() {
    let initial = WorldStateSnapshot::new(
        "epoch",
        0,
        vec![visible_section(
            WorldStateSectionId::EffectivePermissions,
            "ask",
        )],
    )
    .unwrap();
    assert!(matches!(
        WorldStateDiff::from_operations(&initial, 1, vec![]),
        Err(WorldStateError::EmptyDiff)
    ));

    let replacement = visible_section(WorldStateSectionId::EffectivePermissions, "allow");
    let previous = WorldStateSectionPrecondition::from_section(&initial.sections[0]);
    let operation = WorldStateOperation::Replace {
        previous,
        section: replacement,
    };
    assert!(matches!(
        WorldStateDiff::from_operations(&initial, 1, vec![operation.clone(), operation]),
        Err(WorldStateError::DuplicateOperation(
            WorldStateSectionId::EffectivePermissions
        ))
    ));
}

#[test]
fn sequence_cannot_overflow() {
    let snapshot = WorldStateSnapshot::new(
        "epoch",
        u64::MAX,
        vec![visible_section(
            WorldStateSectionId::EffectivePermissions,
            "ask",
        )],
    )
    .unwrap();
    let operation = WorldStateOperation::Remove {
        tombstone: WorldStateSectionTombstone::from_section(&snapshot.sections[0]),
    };

    assert!(matches!(
        WorldStateDiff::from_operations(&snapshot, u64::MAX, vec![operation]),
        Err(WorldStateError::SequenceOverflow)
    ));
}

#[test]
fn durable_record_has_stable_serde_and_accessors() {
    let snapshot = WorldStateSnapshot::new(
        "epoch",
        0,
        vec![visible_section(
            WorldStateSectionId::EffectivePermissions,
            "ask",
        )],
    )
    .unwrap();
    let record = WorldStateRecord::Full(snapshot.clone());
    let json = serde_json::to_string(&record).unwrap();
    let restored: WorldStateRecord = serde_json::from_str(&json).unwrap();

    restored.validate().unwrap();
    assert_eq!(restored.kind(), WorldStateRecordKind::Full);
    assert_eq!(restored.schema_version(), WORLD_STATE_SCHEMA_VERSION);
    assert_eq!(restored.epoch_id(), "epoch");
    assert_eq!(restored.sequence(), 0);
    assert_eq!(restored.base_revision(), None);
    assert_eq!(restored.revision(), snapshot.revision);
    assert_eq!(restored.result_revision(), snapshot.revision);
    assert_eq!(
        restored.canonical_json(),
        WorldStateRecord::Full(snapshot).canonical_json()
    );
}

#[test]
fn anchored_record_validates_opaque_conversation_position() {
    let snapshot = WorldStateSnapshot::new(
        "epoch",
        0,
        vec![visible_section(
            WorldStateSectionId::EffectivePermissions,
            "ask",
        )],
    )
    .unwrap();
    let anchored = AnchoredWorldStateRecord::new(
        WorldStateRecord::Full(snapshot.clone()),
        Some("message-2".to_string()),
    )
    .unwrap();
    let json = serde_json::to_string(&anchored).unwrap();
    let restored: AnchoredWorldStateRecord = serde_json::from_str(&json).unwrap();

    restored.validate().unwrap();
    assert_eq!(
        restored.effective_before_message_id.as_deref(),
        Some("message-2")
    );
    assert!(matches!(
        AnchoredWorldStateRecord::new(WorldStateRecord::Full(snapshot), Some("  ".to_string())),
        Err(WorldStateError::InvalidEffectiveBeforeMessageId)
    ));
}
