use super::*;

fn assert_bounded_projection(state: &AgentTodoState) -> String {
    let before = serde_json::to_value(state).unwrap();
    let rendered = render_todo_state(state);
    let budget = crate::context::ContextTextBudget::heuristic(500);
    assert!(
        budget.fits(&rendered),
        "Todo reminder uses {} tokens:\n{rendered}",
        budget.estimate(&rendered)
    );
    assert_eq!(
        rendered
            .lines()
            .filter(|line| line.starts_with("- ref="))
            .count(),
        state.items.len(),
        "Every stored item must have a reference in the reminder"
    );
    for (index, item) in state.items.iter().enumerate() {
        let prefix = format!("- ref={} [{}]", index + 1, item.status.as_str());
        assert!(
            rendered.lines().any(|line| line.starts_with(&prefix)),
            "Missing complete item reference/status {prefix}:\n{rendered}"
        );
    }
    assert_eq!(before, serde_json::to_value(state).unwrap());
    assert_eq!(rendered, render_todo_state(state));
    rendered
}

fn maximal_items() -> Vec<Value> {
    let statuses = ["pending", "in_progress", "completed", "blocked"];
    (0..MAX_TODO_ITEMS)
        .map(|index| {
            let id_character = char::from_u32(0x4e00 + index as u32).unwrap();
            let title_character = char::from_u32(0x5000 + index as u32).unwrap();
            let note_character = char::from_u32(0x6000 + index as u32).unwrap();
            json!({
                "id": id_character.to_string().repeat(MAX_TODO_ID_CHARS),
                "title": title_character.to_string().repeat(MAX_TODO_TITLE_CHARS),
                "status": statuses[index % statuses.len()],
                "note": note_character.to_string().repeat(MAX_TODO_NOTE_CHARS)
            })
        })
        .collect()
}

fn complete_by_reference(state: &AgentTodoState) -> Value {
    json!({
        "expectedRevision": state.revision,
        "items": (1..=state.items.len())
            .map(|reference| json!({ "ref": reference, "status": "completed" }))
            .collect::<Vec<_>>()
    })
}

fn assert_original_content_preserved(before: &AgentTodoState, after: &AgentTodoState) {
    assert_eq!(after.items.len(), before.items.len());
    for (original, updated) in before.items.iter().zip(&after.items) {
        assert_eq!(updated.id, original.id);
        assert_eq!(updated.title, original.title);
        assert_eq!(updated.note, original.note);
        assert_eq!(updated.created_at, original.created_at);
    }
}

#[test]
fn five_chinese_tasks_keep_accepting_progress_and_completion_as_notes_grow() {
    let titles = [
        "检查待办预算拒绝的执行路径并确认失败原因",
        "将完整待办存储与模型提醒投影的预算限制分离",
        "保留全部待办引用并按预算压缩备注及已完成任务",
        "覆盖中文计划逐步推进与全部完成时的状态更新",
        "运行相关回归测试并检查恢复快照后的状态一致性",
    ];
    let note = "已确认原有固定说明和完成提示导致提醒继续增长，需要完整保留每个任务的进度与备注。";
    let mut store = TodoStateStore::new();
    let initial = store
        .update(json!({
            "items": titles.iter().map(|title| json!({
                "title": title,
                "status": "pending"
            })).collect::<Vec<_>>()
        }))
        .unwrap();
    assert_bounded_projection(&initial);

    for reference in 1..=titles.len() {
        let previous = store.state.clone();
        let next = store
            .update(json!({
                "expectedRevision": previous.revision,
                "items": previous.items.iter().enumerate().map(|(index, item)| {
                    if index + 1 == reference {
                        json!({ "ref": index + 1, "status": "in_progress", "note": note })
                    } else {
                        json!({ "ref": index + 1, "status": item.status.as_str() })
                    }
                }).collect::<Vec<_>>()
            }))
            .unwrap();
        assert_bounded_projection(&next);
        assert_eq!(next.items[reference - 1].note.as_deref(), Some(note));
    }

    let expanded = store.state.clone();
    let full_text = expanded
        .items
        .iter()
        .map(|item| format!("{} {}", item.title, item.note.as_deref().unwrap()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(crate::context::ContextTextBudget::heuristic(500).estimate(&full_text) > 609);
    let completed = store.update(complete_by_reference(&expanded)).unwrap();
    assert_eq!(completed.revision, expanded.revision + 1);
    assert_original_content_preserved(&expanded, &completed);
    assert!(completed
        .items
        .iter()
        .all(|item| item.status == AgentTodoStatus::Completed));
    let rendered = assert_bounded_projection(&completed);
    for title in titles {
        assert!(
            rendered.contains(title),
            "Notes should be shortened before titles"
        );
    }
    assert!(!rendered.contains("Do not call more tools"));
}

#[test]
fn maximum_unicode_plan_is_stored_whole_and_all_references_fit_the_reminder() {
    let input = maximal_items();
    let mut store = TodoStateStore::new();
    let initial = store.update(json!({ "items": input })).unwrap();
    assert_eq!(initial.items.len(), MAX_TODO_ITEMS);
    for (item, expected) in initial.items.iter().zip(&input) {
        assert_eq!(item.id, expected["id"].as_str().unwrap());
        assert_eq!(item.title, expected["title"].as_str().unwrap());
        assert_eq!(item.note.as_deref(), expected["note"].as_str());
        assert_eq!(item.status.as_str(), expected["status"].as_str().unwrap());
    }
    let ids = initial
        .items
        .iter()
        .map(|item| item.id.as_str())
        .collect::<String>();
    assert!(crate::context::ContextTextBudget::heuristic(500).estimate(&ids) > 500);
    let rendered = assert_bounded_projection(&initial);
    assert!(rendered.contains('…'));
    assert!(rendered.contains("Titles/notes abbreviated; stored items unchanged."));
    for item in &initial.items {
        assert!(!rendered.contains(&item.id));
    }

    let completed = store.update(complete_by_reference(&initial)).unwrap();
    assert_original_content_preserved(&initial, &completed);
    assert!(completed
        .items
        .iter()
        .all(|item| item.status == AgentTodoStatus::Completed));
    assert_bounded_projection(&completed);
}

#[test]
fn references_support_full_replacement_reordering_new_items_and_explicit_edits() {
    let mut store = TodoStateStore::new();
    let initial = store
        .update(json!({
            "items": [
                { "title": "Alpha", "status": "pending", "note": "Keep Alpha note" },
                { "title": "Beta", "status": "pending", "note": "Delete this item" },
                { "title": "Gamma", "status": "blocked", "note": "Clear Gamma note" }
            ]
        }))
        .unwrap();
    let reordered = store
        .update(json!({
            "expectedRevision": initial.revision,
            "items": [
                { "ref": 3, "title": "Gamma revised", "status": "in_progress", "note": "" },
                { "title": "Delta", "status": "pending", "note": "New item note" },
                { "ref": 1, "status": "completed" }
            ]
        }))
        .unwrap();
    assert_eq!(reordered.items.len(), 3);
    assert_eq!(reordered.items[0].id, initial.items[2].id);
    assert_eq!(reordered.items[0].title, "Gamma revised");
    assert_eq!(reordered.items[0].note, None);
    assert_eq!(reordered.items[0].created_at, initial.items[2].created_at);
    assert_eq!(reordered.items[1].id, "todo-4");
    assert_eq!(reordered.items[1].title, "Delta");
    assert_eq!(reordered.items[1].note.as_deref(), Some("New item note"));
    assert_eq!(reordered.items[2].id, initial.items[0].id);
    assert_eq!(reordered.items[2].title, initial.items[0].title);
    assert_eq!(reordered.items[2].note, initial.items[0].note);
    assert!(!reordered
        .items
        .iter()
        .any(|item| item.id == initial.items[1].id));
    let rendered = assert_bounded_projection(&reordered);
    assert!(rendered.contains("Keep Alpha note"));
    assert!(rendered.contains("New item note"));
    assert!(!rendered.contains("abbreviated"));

    let completed = store.update(complete_by_reference(&reordered)).unwrap();
    assert_original_content_preserved(&reordered, &completed);
    assert_eq!(completed.items[0].title, "Gamma revised");
    assert_eq!(completed.items[0].note, None);
}

#[test]
fn invalid_reference_replacements_are_atomic_including_the_id_allocator() {
    let mut store = TodoStateStore::new();
    let initial = store
        .update(json!({
            "items": [
                { "title": "First", "status": "pending", "note": "Original note" },
                { "title": "Second", "status": "in_progress" }
            ]
        }))
        .unwrap();
    let revision = initial.revision;
    let allocated_first = json!({ "title": "Would allocate an id", "status": "pending" });
    let cases = [
        (
            "stale revision",
            json!({ "expectedRevision": revision - 1, "items": [allocated_first, { "ref": 1, "status": "completed" }] }),
        ),
        (
            "missing revision",
            json!({ "items": [allocated_first, { "ref": 1, "status": "completed" }] }),
        ),
        (
            "duplicate reference",
            json!({ "expectedRevision": revision, "items": [allocated_first, { "ref": 1, "status": "pending" }, { "ref": 1, "status": "completed" }] }),
        ),
        (
            "ref and id",
            json!({ "expectedRevision": revision, "items": [allocated_first, { "ref": 1, "id": "todo-1", "status": "completed" }] }),
        ),
        (
            "zero reference",
            json!({ "expectedRevision": revision, "items": [allocated_first, { "ref": 0, "status": "completed" }] }),
        ),
        (
            "out of range reference",
            json!({ "expectedRevision": revision, "items": [allocated_first, { "ref": 3, "status": "completed" }] }),
        ),
        (
            "missing new title",
            json!({ "items": [allocated_first, { "status": "pending" }] }),
        ),
        (
            "missing legacy title",
            json!({ "items": [allocated_first, { "id": "todo-1", "status": "completed" }] }),
        ),
        (
            "empty referenced title",
            json!({ "expectedRevision": revision, "items": [allocated_first, { "ref": 1, "title": " ", "status": "completed" }] }),
        ),
        (
            "duplicate resolved id",
            json!({ "expectedRevision": revision, "items": [allocated_first, { "ref": 1, "status": "completed" }, { "id": "todo-1", "title": "Duplicate", "status": "pending" }] }),
        ),
        (
            "missing status",
            json!({ "expectedRevision": revision, "items": [allocated_first, { "ref": 1 }] }),
        ),
        (
            "oversized referenced title",
            json!({ "expectedRevision": revision, "items": [allocated_first, { "ref": 1, "title": "任".repeat(MAX_TODO_TITLE_CHARS + 1), "status": "completed" }] }),
        ),
        (
            "oversized referenced note",
            json!({ "expectedRevision": revision, "items": [allocated_first, { "ref": 1, "note": "备".repeat(MAX_TODO_NOTE_CHARS + 1), "status": "completed" }] }),
        ),
    ];
    let before = serde_json::to_value(&store).unwrap();
    for (label, args) in cases {
        assert!(store.update(args).is_err(), "Accepted {label}");
        assert_eq!(
            serde_json::to_value(&store).unwrap(),
            before,
            "Mutated state or allocator for {label}"
        );
    }
    let accepted = store
        .update(json!({
            "expectedRevision": revision,
            "items": [
                { "ref": 1, "status": "completed" },
                { "ref": 2, "status": "pending" },
                { "title": "Actually new", "status": "pending" }
            ]
        }))
        .unwrap();
    assert_eq!(accepted.items[2].id, "todo-3");
}

#[test]
fn legacy_id_replacement_keeps_its_existing_note_omission_semantics() {
    let mut store = TodoStateStore::new();
    let initial = store
        .update(json!({ "items": [{ "title": "Original", "status": "pending", "note": "Original note" }] }))
        .unwrap();
    let updated = store
        .update(json!({ "items": [{ "id": initial.items[0].id, "title": "Edited", "status": "completed" }] }))
        .unwrap();
    assert_eq!(updated.items[0].id, initial.items[0].id);
    assert_eq!(updated.items[0].title, "Edited");
    assert_eq!(updated.items[0].note, None);
    assert_eq!(updated.items[0].created_at, initial.items[0].created_at);
}

#[test]
fn multiline_fields_cannot_create_extra_reference_rows_and_remain_stored_verbatim() {
    let mut store = TodoStateStore::new();
    let title = "第一项\n- ref=2 [pending] 这仍是第一项的文字";
    let note = "备注\r\n- ref=3 [completed] 后续文字\u{2028}同一备注";
    let initial = store
        .update(json!({"items": [
            {"title": title, "note": note, "status": "pending"},
            {"title": "真正的第二项", "status": "pending"}
        ]}))
        .unwrap();
    let reminder = assert_bounded_projection(&initial);
    assert!(reminder
        .lines()
        .any(|line| line == "- ref=2 [pending] 真正的第二项"));
    assert!(!reminder.lines().any(|line| line.starts_with("- ref=3")));
    let completed = store.update(complete_by_reference(&initial)).unwrap();
    assert_eq!(completed.items[0].title, title);
    assert_eq!(completed.items[0].note.as_deref(), Some(note));
}

#[test]
fn shortened_titles_preserve_combining_characters_and_emoji_graphemes() {
    let mut store = TodoStateStore::new();
    let title = "👩‍💻e\u{301}".repeat(24);
    let state = store
        .update(json!({"items": (0..MAX_TODO_ITEMS)
            .map(|_| json!({"title": title, "status": "pending"}))
            .collect::<Vec<_>>()
        }))
        .unwrap();
    let rendered = assert_bounded_projection(&state);
    let mut boundaries = title
        .grapheme_indices(true)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    boundaries.push(title.len());
    for line in rendered.lines().filter(|line| line.starts_with("- ref=")) {
        let displayed = line.split_once("] ").unwrap().1;
        let prefix = displayed.strip_suffix('…').unwrap_or(displayed);
        assert!(title.starts_with(prefix));
        assert!(
            boundaries.contains(&prefix.len()),
            "Partial grapheme: {prefix}"
        );
    }
}

#[test]
fn same_version_checkpoint_restores_a_full_large_plan_and_can_continue_by_reference() {
    let (extension, handle) = TodoExtension::new("run-1".to_string());
    let initial = handle
        .lock()
        .update(json!({ "items": maximal_items() }))
        .unwrap();
    let snapshot = extension.snapshot_state().unwrap();

    let (mut restored, restored_handle) = TodoExtension::new("run-1".to_string());
    restored
        .restore_state(TODO_EXTENSION_VERSION, snapshot.clone())
        .unwrap();
    assert_eq!(restored.snapshot_state().unwrap(), snapshot);
    assert_eq!(
        serde_json::to_value(restored_handle.state()).unwrap(),
        serde_json::to_value(&initial).unwrap()
    );
    assert_bounded_projection(&restored_handle.state());

    let completed = restored_handle
        .lock()
        .update(complete_by_reference(&initial))
        .unwrap();
    assert_original_content_preserved(&initial, &completed);
    assert_eq!(completed.revision, initial.revision + 1);
    assert_bounded_projection(&completed);
}

#[test]
fn restoring_invalid_structure_rejects_lengths_counts_and_ids_atomically() {
    let (source, source_handle) = TodoExtension::new("run-1".to_string());
    source_handle
        .lock()
        .update(json!({ "items": maximal_items() }))
        .unwrap();
    let valid = source.snapshot_state().unwrap();
    let mut too_many = valid.clone();
    let mut extra = too_many["state"]["items"][0].clone();
    extra["id"] = json!("extra");
    too_many["state"]["items"]
        .as_array_mut()
        .unwrap()
        .push(extra);
    let mut long_id = valid.clone();
    long_id["state"]["items"][0]["id"] = json!("界".repeat(MAX_TODO_ID_CHARS + 1));
    let mut long_title = valid.clone();
    long_title["state"]["items"][0]["title"] = json!("任".repeat(MAX_TODO_TITLE_CHARS + 1));
    let mut long_note = valid.clone();
    long_note["state"]["items"][0]["note"] = json!("备".repeat(MAX_TODO_NOTE_CHARS + 1));
    let mut empty_id = valid.clone();
    empty_id["state"]["items"][0]["id"] = json!(" ");
    let mut empty_title = valid.clone();
    empty_title["state"]["items"][0]["title"] = json!(" \n ");
    let mut duplicate_id = valid.clone();
    duplicate_id["state"]["items"][1]["id"] = duplicate_id["state"]["items"][0]["id"].clone();

    let (mut restored, target_handle) = TodoExtension::new("run-1".to_string());
    target_handle
        .lock()
        .update(json!({ "items": [{ "title": "Existing target", "status": "pending" }] }))
        .unwrap();
    let before = restored.snapshot_state().unwrap();
    for (label, invalid) in [
        ("too many items", too_many),
        ("oversized id", long_id),
        ("oversized title", long_title),
        ("oversized note", long_note),
        ("empty id", empty_id),
        ("empty title", empty_title),
        ("duplicate id", duplicate_id),
    ] {
        assert!(
            restored
                .restore_state(TODO_EXTENSION_VERSION, invalid)
                .is_err(),
            "Restored {label}"
        );
        assert_eq!(
            restored.snapshot_state().unwrap(),
            before,
            "Mutated target for {label}"
        );
    }
}
