use serde_json::{json, Value};

/// Model wording only. The host's policy and capability IDs remain authoritative and unchanged.
/// Apply the same view to newly prepared sections and retained full/diff records.
pub(crate) fn for_model(projection: &Value) -> Value {
    let available = projection
        .get("subagentToolsAvailable")
        .or_else(|| projection.get("available"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let reason = if available {
        "subagents_available"
    } else if matches!(
        projection.get("reason").and_then(Value::as_str),
        Some("disabled_by_user" | "subagents_disabled_for_this_run")
    ) {
        "subagents_disabled_for_this_run"
    } else {
        "subagent_host_unavailable"
    };
    json!({
        "subagentToolsAvailable": available,
        "effectiveScope": "current_run",
        "reason": reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_state::{
        WorldStateDiff, WorldStateLifetime, WorldStateReducer, WorldStateSectionEnvelope,
        WorldStateSectionId, WorldStateSnapshot,
    };

    fn snapshot(sequence: u64, policy_revision: u64, available: bool) -> WorldStateSnapshot {
        let projection = json!({
            "enabled": available, "available": available,
            "reason": if available { "available" } else { "disabled_by_user" },
        });
        let mut state = projection.clone();
        state["policyRevision"] = json!(policy_revision);
        WorldStateSnapshot::new(
            "epoch",
            sequence,
            vec![WorldStateSectionEnvelope::model_visible(
                WorldStateSectionId::extension("agent.collaboration").unwrap(),
                WorldStateLifetime::Conversation,
                state,
                projection,
            )
            .unwrap()],
        )
        .unwrap()
    }

    #[test]
    fn subagent_projection_distinguishes_disabled_from_missing_host_without_generic_switches() {
        for (raw, available, reason) in [
            (
                json!({"enabled":true,"available":true,"reason":"available"}),
                true,
                "subagents_available",
            ),
            (
                json!({"enabled":false,"available":false,"reason":"disabled_by_user"}),
                false,
                "subagents_disabled_for_this_run",
            ),
            (
                json!({"enabled":true,"available":false,"reason":"host_unavailable"}),
                false,
                "subagent_host_unavailable",
            ),
        ] {
            let projected = for_model(&raw);
            assert_eq!(
                projected,
                json!({
                    "subagentToolsAvailable":available,
                    "effectiveScope":"current_run",
                    "reason":reason,
                })
            );
            assert_eq!(for_model(&projected), projected);
        }
    }

    #[test]
    fn retained_full_diff_and_compaction_rebase_use_scoped_fields_without_rewriting_host_state() {
        let before = snapshot(0, 1, false);
        let original = before.canonical_json();
        let restored: WorldStateSnapshot = serde_json::from_str(&original).unwrap();
        let lifetime = WorldStateLifetime::Conversation;
        for state in [restored.clone(), restored.rebase("compacted").unwrap()] {
            let text = state
                .model_projection(lifetime)
                .unwrap()
                .render_sanitized_text();
            assert!(text.contains("\"subagentToolsAvailable\":false"));
            assert!(text.contains("subagents_disabled_for_this_run"));
            assert!(!text.contains("\"enabled\""));
            assert!(!text.contains("\"available\""));
            assert!(!text.contains("policyRevision"));
            assert!(state.canonical_json().contains("disabled_by_user"));
            state.validate().unwrap();
        }
        assert_eq!(before.canonical_json(), original);
        // The old display remains available only for exact retained-context validation.
        assert!(before
            .stored_model_projection_for_validation(lifetime)
            .unwrap()
            .render_sanitized_text()
            .contains("\"enabled\":false"));

        let after = snapshot(1, 2, true);
        let diff = WorldStateDiff::between(&before, &after).unwrap();
        let text = diff
            .model_projection_against(&before, lifetime)
            .unwrap()
            .unwrap()
            .render_sanitized_text();
        assert!(text.contains("\"subagentToolsAvailable\":true"));
        assert!(text.contains("\"effectiveScope\":\"current_run\""));
        assert!(!text.contains("\"enabled\"") && !text.contains("\"available\""));
        assert_eq!(WorldStateReducer::fold(before, &[diff]).unwrap(), after);
    }

    #[test]
    fn projection_upgrade_and_policy_metadata_do_not_repeat_unchanged_capability_state() {
        let before = snapshot(0, 1, false);
        let mut sections = before.sections.clone();
        let original = &sections[0];
        sections[0] = WorldStateSectionEnvelope::model_visible(
            original.id.clone(), original.lifetime,
            json!({"enabled":false,"available":false,"reason":"disabled_by_user","policyRevision":2}),
            for_model(original.model_projection.as_ref().unwrap()),
        ).unwrap();
        let after = WorldStateSnapshot::new("epoch", 1, sections).unwrap();
        assert_ne!(before.revision, after.revision);
        assert_eq!(
            WorldStateDiff::between(&before, &after)
                .unwrap()
                .model_projection_against(&before, WorldStateLifetime::Conversation)
                .unwrap(),
            None
        );
    }
}
