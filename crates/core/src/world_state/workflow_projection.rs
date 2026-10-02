use serde_json::Value;

/// Template provenance is Host bookkeeping, not part of the collaborator's working context.
/// Apply the same policy to newly published sections and stored journal projections at render
/// time, without rewriting authoritative state, journal revisions or checkpoint records.
pub(crate) fn for_model(section_id: &str, mut projection: Value) -> Value {
    let metadata = match section_id {
        "workflow.execution" => projection.get_mut("workflow"),
        "workflow.awareness" => Some(&mut projection),
        _ => None,
    };
    if let Some(metadata) = metadata.and_then(Value::as_object_mut) {
        for field in ["templateId", "templateRevision", "executionVersion"] {
            metadata.remove(field);
        }
    }
    projection
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use serde_json::json;

    fn stored_snapshot(sequence: u64, version: u64, task: &str) -> WorldStateSnapshot {
        let execution = json!({"available":true,"workflow":{
            "instanceId":"team","name":"Mail team","templateId":format!("template-{version}"),
            "templateRevision":version,"executionVersion":format!("execution-{version}"),
            "nodeId":"reviewer","nodeName":"Reviewer","background":"Shared background",
            "receives":"Changes","task":task,"delivers":"Review results",
            "members":[{"nodeId":"writer","nodeName":"Writer","task":"Write"}],"enabled":true
        }});
        let awareness = json!({"available":true,"instanceId":"team","executionVersion":format!("execution-{version}"),
            "currentNodeId":"reviewer","nodes":[{"nodeId":"writer","state":"running"}]});
        // Intentionally retain the original unfiltered projections: these represent an existing
        // persisted journal, so tests cover replay rather than only new extension publications.
        WorldStateSnapshot::new(
            "epoch",
            sequence,
            [
                ("workflow.execution", execution),
                ("workflow.awareness", awareness),
            ]
            .into_iter()
            .map(|(id, value)| {
                WorldStateSectionEnvelope::model_visible(
                    WorldStateSectionId::extension(id).unwrap(),
                    WorldStateLifetime::Conversation,
                    value.clone(),
                    value,
                )
                .unwrap()
            })
            .collect(),
        )
        .unwrap()
    }

    fn assert_no_template_metadata(text: &str) {
        for field in [
            "templateId",
            "templateRevision",
            "executionVersion",
            "template-1",
            "execution-1",
        ] {
            assert!(!text.contains(field), "model projection exposed {field}");
        }
    }

    #[test]
    fn workflow_replay_and_rebase_hide_template_metadata_without_changing_host_state() {
        let stored = stored_snapshot(0, 1, "Review changes");
        let canonical = stored.canonical_json();
        let restored: WorldStateSnapshot = serde_json::from_str(&canonical).unwrap();
        for snapshot in [
            restored.clone(),
            restored.rebase("compacted-epoch").unwrap(),
        ] {
            let text = snapshot
                .model_projection(WorldStateLifetime::Conversation)
                .unwrap()
                .render_sanitized_text();
            assert_no_template_metadata(&text);
            for useful in [
                "Mail team",
                "Shared background",
                "Changes",
                "Review changes",
                "Review results",
                "writer",
                "running",
            ] {
                assert!(text.contains(useful), "working context lost {useful}");
            }
            assert!(snapshot.canonical_json().contains("templateId"));
            snapshot.validate().unwrap();
        }
        assert_eq!(stored.canonical_json(), canonical);
    }

    #[test]
    fn workflow_internal_version_changes_do_not_create_model_diffs() {
        let before = stored_snapshot(0, 1, "Review changes");
        let after = stored_snapshot(1, 2, "Review changes");
        let diff = WorldStateDiff::between(&before, &after).unwrap();
        assert_ne!(before.revision, after.revision);
        assert_eq!(
            before
                .model_projection_revision(WorldStateLifetime::Conversation)
                .unwrap(),
            after
                .model_projection_revision(WorldStateLifetime::Conversation)
                .unwrap()
        );
        assert_eq!(
            diff.model_projection_against(&before, WorldStateLifetime::Conversation)
                .unwrap(),
            None
        );
        let changed_task = stored_snapshot(2, 3, "Review final draft");
        let diff = WorldStateDiff::between(&after, &changed_task).unwrap();
        let text = diff
            .model_projection_against(&after, WorldStateLifetime::Conversation)
            .unwrap()
            .unwrap()
            .render_sanitized_text();
        assert_no_template_metadata(&text);
        assert!(text.contains("Review final draft"));
        assert!(!text.contains("execution-3"));
        assert!(!text.contains("template-3"));
    }
}
