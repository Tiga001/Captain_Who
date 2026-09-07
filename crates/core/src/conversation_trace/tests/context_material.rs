fn context_material_fixture() -> ConversationTurnTrace {
    serde_json::from_str(include_str!(
        "../../../../../packages/protocol/fixtures/conversation-context-material-v1.json"
    ))
    .unwrap()
}

#[test]
fn context_material_records_exact_atomic_idempotent_model_and_trace_facts() {
    let fixture = context_material_fixture();
    fixture.validate().unwrap();
    let mut recorder = ConversationTraceRecorder::default();
    for item in &fixture.items {
        let ConversationTurnTraceItem::ContextMaterial {
            sequence,
            event_id,
            material_kind,
            content,
            images,
            created_at,
        } = item
        else {
            unreachable!()
        };
        for _ in 0..2 {
            assert_eq!(
                recorder
                    .record_context_material(event_id, *material_kind, content, images, *created_at)
                    .unwrap(),
                *sequence
            );
        }
        assert!(item.is_model_visible());
        assert!(item.is_safe_compaction_boundary());
    }
    let snapshot = recorder.snapshot();
    assert_eq!(snapshot.items, fixture.items);
    assert_eq!(snapshot.items.len(), snapshot.model_context_items.len());
    fixture
        .validate_complete_model_context(&snapshot.model_context_items)
        .unwrap();
    for (trace, model) in fixture.items.iter().zip(&snapshot.model_context_items) {
        let ConversationTurnTraceItem::ContextMaterial {
            content, images, ..
        } = trace
        else {
            unreachable!()
        };
        assert_eq!(&model.content, content);
        assert_eq!(&model.images, images);
        assert_eq!(model.role, "user");
    }
    // A same-version restart reuses both journals without rebuilding or duplicating material.
    let mut restored = ConversationTraceRecorder::from_durable_snapshot(snapshot);
    let ConversationTurnTraceItem::ContextMaterial {
        event_id,
        material_kind,
        content,
        images,
        created_at,
        ..
    } = &fixture.items[0]
    else {
        unreachable!()
    };
    let before = restored.checkpoint();
    assert_eq!(
        restored
            .record_context_material(event_id, *material_kind, content, images, *created_at)
            .unwrap(),
        0
    );
    assert!(restored
        .record_context_material(event_id, *material_kind, "changed", images, *created_at)
        .is_err());
    assert!(restored
        .record_context_material(event_id, *material_kind, content, images, *created_at + 1)
        .is_err());
    assert_eq!(restored.checkpoint(), before);
}

#[test]
fn context_material_rejects_partial_or_forged_journal_bindings() {
    let fixture = context_material_fixture();
    let mut recorder = ConversationTraceRecorder::default();
    let ConversationTurnTraceItem::ContextMaterial {
        event_id,
        material_kind,
        content,
        images,
        created_at,
        ..
    } = &fixture.items[0]
    else {
        unreachable!()
    };
    recorder
        .record_context_material(event_id, *material_kind, content, images, *created_at)
        .unwrap();
    let snapshot = recorder.snapshot();
    let trace = snapshot.in_progress_audit_trace("run", "conversation", "assistant");
    for mutation in ["image_hash", "image_id", "content", "role", "ordinal"] {
        let mut model = snapshot.model_context_items.clone();
        match mutation {
            "image_hash" => model[0].images[0].sha256 = format!("sha256:{}", "b".repeat(64)),
            "image_id" => model[0].images[0].attachment_id = "different-image".to_string(),
            "content" => model[0].content.push('!'),
            "role" => model[0].role = "assistant".to_string(),
            "ordinal" => model[0].ordinal = 1,
            _ => unreachable!(),
        }
        assert!(
            trace.validate_complete_model_context(&model).is_err(),
            "{mutation}"
        );
    }
    let mut duplicate = trace.clone();
    let mut second = duplicate.items[0].clone();
    if let ConversationTurnTraceItem::ContextMaterial { sequence, .. } = &mut second {
        *sequence = 1;
    }
    duplicate.items.push(second);
    assert!(duplicate.validate().is_err());
    let mut broken = ConversationTraceRecorder::from_durable_snapshot(snapshot);
    broken.model_context_items.clear();
    let before = broken.checkpoint();
    assert!(broken
        .record_context_material(event_id, *material_kind, content, images, *created_at)
        .is_err());
    assert_eq!(broken.checkpoint(), before);
}

#[test]
fn context_material_rejects_unsafe_input_without_consuming_a_sequence() {
    let fixture = context_material_fixture();
    let item = serde_json::to_value(&fixture.items[0]).unwrap();
    for (key, value) in [
        ("eventId", json!("")),
        ("eventId", json!("x\ny")),
        ("createdAt", json!(-1)),
        ("content", json!("data:image/png;base64,AAAA")),
        (
            "content",
            json!("界".repeat(MAX_CONTEXT_MATERIAL_CONTENT_BYTES / 3 + 1)),
        ),
        ("materialKind", json!("skill_instructions")),
        (
            "images",
            json!([{"attachmentId":"a","mimeType":"image/png","sha256":"bad"}]),
        ),
    ] {
        let mut malformed = item.clone();
        malformed[key] = value;
        let item: ConversationTurnTraceItem = serde_json::from_value(malformed).unwrap();
        let ConversationTurnTraceItem::ContextMaterial {
            event_id,
            material_kind,
            content,
            images,
            created_at,
            ..
        } = item
        else {
            unreachable!()
        };
        let mut recorder = ConversationTraceRecorder::default();
        assert!(
            recorder
                .record_context_material(&event_id, material_kind, &content, &images, created_at)
                .is_err(),
            "{key}"
        );
        assert_eq!(recorder.next_sequence(), 0);
        assert!(recorder.snapshot().items.is_empty());
    }
    let mut unknown = item.clone();
    unknown["approved"] = json!(true);
    assert!(serde_json::from_value::<ConversationTurnTraceItem>(unknown).is_err());
    let mut binary = item;
    binary["images"][0]["bytes"] = json!("AAAA");
    assert!(serde_json::from_value::<ConversationTurnTraceItem>(binary).is_err());
    let mut recorder = ConversationTraceRecorder::default();
    record_open_call(&mut recorder, &call("pending-call"));
    let before = recorder.checkpoint();
    assert!(recorder
        .record_context_material(
            "later",
            ConversationContextMaterialKind::RunWorldState,
            "state",
            &[],
            1
        )
        .is_err());
    assert_eq!(recorder.checkpoint(), before);
}

#[test]
fn context_material_images_are_optional_and_narration_keeps_provider_turn_identity() {
    let item = ConversationModelContextItem {
        sequence: 0,
        ordinal: 0,
        role: "user".to_string(),
        content: "text".to_string(),
        images: Vec::new(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        is_error: false,
    };
    let wire = serde_json::to_value(&item).unwrap();
    assert!(wire.get("images").is_none());
    assert_eq!(
        serde_json::from_value::<ConversationModelContextItem>(wire).unwrap(),
        item
    );
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_narration("Same text").unwrap();
    let first_call = crate::llm::model_response_tool_call_id("run", 0, 0, "first-call");
    recorder
        .record_tool_turn_narration("Same text", "at1_provider_turn", &first_call)
        .unwrap();
    let snapshot = recorder.snapshot();
    assert!(matches!(
        &snapshot.items[0],
        ConversationTurnTraceItem::AssistantNarration {
            provider_turn_id: None,
            first_tool_call_id: None,
            ..
        }
    ));
    assert!(
        matches!(&snapshot.items[1], ConversationTurnTraceItem::AssistantNarration { provider_turn_id: Some(id), .. } if id == "at1_provider_turn")
    );
    let before = recorder.checkpoint();
    assert!(recorder
        .record_tool_turn_narration("content", "", &first_call)
        .is_err());
    assert_eq!(recorder.checkpoint(), before);
}
