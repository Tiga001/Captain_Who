use super::*;

fn sample(schema: &Value) -> Value {
    if let Some(first) = schema["oneOf"].as_array().and_then(|items| items.first()) {
        return sample(first);
    }
    if let Some(first) = schema["enum"].as_array().and_then(|items| items.first()) {
        return first.clone();
    }
    match schema["type"].as_str().unwrap() {
        "object" => Value::Object(
            required(schema)
                .into_iter()
                .map(|key| (key.to_string(), sample(&properties(schema)[key])))
                .collect(),
        ),
        "array" => json!([sample(&schema["items"])]),
        "string" => json!(if schema["minLength"].as_u64().unwrap_or(0) > 0 {
            "sample"
        } else {
            ""
        }),
        "integer" => json!(0),
        "boolean" => json!(false),
        _ => unreachable!(),
    }
}

fn request_branches() -> Vec<&'static Value> {
    branches(&schema::contract_schema()["properties"]["request"])
}

fn valid_update() -> Value {
    json!({"request":{"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","edits":[{"kind":"replace","oldText":"before","newText":"after"}]}})
}

fn diagnostics(value: &Value) -> Value {
    let error = super::super::validate_wire_shape(value).expect_err("invalid wire");
    super::super::file_change_wire_error(error, value)
        .details()
        .unwrap()
        .clone()
}

#[test]
fn every_published_branch_is_accepted_and_deserializes() {
    let all = request_branches();
    assert_eq!(all.len(), 11);
    for schema in all {
        let value = json!({"request":sample(schema)});
        assert!(
            inspect(&value).is_empty(),
            "{value:?}: {:?}",
            inspect(&value)
        );
        super::super::parse_args(value.clone()).expect("schema and serde agree");
        for key in required(schema) {
            let mut missing = value.clone();
            missing["request"].as_object_mut().unwrap().remove(key);
            assert!(
                !inspect(&missing).is_empty(),
                "missing {key} was accepted: {missing}"
            );
        }
    }
    let update = request_branches()
        .into_iter()
        .find(|schema| {
            properties(schema).contains_key("edits")
                && schema["properties"]["action"]["enum"] == json!(["apply"])
        })
        .unwrap();
    for edit in branches(&update["properties"]["edits"]["items"]) {
        let mut value = valid_update();
        value["request"]["edits"] = json!([sample(edit)]);
        assert!(inspect(&value).is_empty());
        super::super::parse_args(value).unwrap();
    }
}

#[test]
fn common_missing_fields_are_localized_without_repairing_input() {
    let value = json!({"request":{"action":"apply","edits":[{"kind":"replace","oldText":"PRIVATE_BODY","newText":"PRIVATE_REPLACEMENT"}]}});
    let before = value.clone();
    let details = diagnostics(&value);
    let pointers: Vec<_> = details["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|issue| issue["pointer"].as_str())
        .collect();
    assert!(pointers.contains(&"/request/operation"));
    assert!(pointers.contains(&"/request/filePath"));
    assert!(pointers.contains(&"/request/observationId"));
    assert_eq!(details["outcome"], "definitely_not_executed");
    assert!(!details.to_string().contains("PRIVATE_"));
    assert_eq!(value, before);
    assert_eq!(details["expectedShape"]["request"]["operation"], "update");
}

#[test]
fn missing_payload_never_suggests_delete() {
    let value =
        json!({"request":{"action":"apply","filePath":"a.txt","observationId":"fobs_example"}});
    let shape = expected_shape(&value);
    assert!(shape["request"]["operation"].is_null());
    assert!(shape["request"]["allowedOperations"]
        .as_array()
        .unwrap()
        .contains(&json!("update")));
    assert!(inspect(&value)
        .iter()
        .any(|issue| issue.details["pointer"] == "/request/operation"));
}

#[test]
fn missing_discriminators_include_the_schema_enum_choices() {
    let mut value = valid_update();
    value["request"]["edits"][0]
        .as_object_mut()
        .unwrap()
        .remove("kind");
    let issues = inspect(&value);
    let missing = issues
        .iter()
        .find(|issue| issue.details["pointer"] == "/request/edits/0/kind")
        .unwrap();
    assert_eq!(
        missing.details["allowedValues"],
        json!([
            "replace",
            "insert_before",
            "insert_after",
            "append",
            "prepend"
        ])
    );
    value["request"].as_object_mut().unwrap().remove("action");
    value["request"]
        .as_object_mut()
        .unwrap()
        .remove("operation");
    let issues = inspect(&value);
    let missing_action = issues
        .iter()
        .find(|issue| issue.details["pointer"] == "/request/action")
        .unwrap();
    assert!(missing_action.details["allowedValues"]
        .as_array()
        .unwrap()
        .contains(&json!("apply")));
}

#[test]
fn nested_fields_and_misplaced_summary_get_exact_pointers() {
    let mut value = valid_update();
    value["summary"] = json!("PRIVATE_ROOT_SUMMARY");
    value["request"]["edits"][0] = json!({"kind":"insert_before","anchor":"PRIVATE_ANCHOR","newText":"PRIVATE_BODY","extra_name":"PRIVATE_EXTRA"});
    let details = diagnostics(&value);
    let issues = details["diagnostics"].as_array().unwrap();
    for pointer in [
        "/summary",
        "/request/edits/0/text",
        "/request/edits/0/newText",
        "/request/edits/0/extra_name",
    ] {
        assert!(
            issues.iter().any(|issue| issue["pointer"] == pointer),
            "missing {pointer}: {issues:?}"
        );
    }
    assert!(!details.to_string().contains("PRIVATE_"));
    let shapes = details["expectedShape"]["request"]["editShapes"]
        .as_object()
        .unwrap();
    assert_eq!(shapes.len(), 1);
    assert!(shapes.contains_key("insert_before"));
}

#[test]
fn utf8_limits_report_bytes_not_characters_and_recover_safely() {
    let mut value = json!({"request":{"action":"apply","operation":"create","filePath":"a.txt","content":"界".repeat(MAX_INLINE_CONTENT_BYTES / 3)}});
    assert!(inspect(&value).is_empty());
    value["request"]["content"] = json!("界".repeat(MAX_INLINE_CONTENT_BYTES / 3 + 1));
    let details = diagnostics(&value);
    let issue = &details["diagnostics"][0];
    assert_eq!(issue["pointer"], "/request/content");
    assert_eq!(issue["reason"], "utf8_byte_limit");
    assert_eq!(issue["maxBytes"], MAX_INLINE_CONTENT_BYTES);
    assert_eq!(issue["actualBytes"], (MAX_INLINE_CONTENT_BYTES / 3 + 1) * 3);
    assert_eq!(
        details["continueWith"]["args"]["request"]["action"],
        "begin"
    );
    super::super::validate_wire_shape(&details["continueWith"]["args"]).unwrap();
    assert!(!details.to_string().contains('界'));
}

#[test]
fn oversized_utf8_append_preserves_the_transaction_and_corrects_only_the_chunk() {
    let max_bytes = file_change_staged::MAX_STAGED_CHUNK_BYTES;
    let boundary = format!(
        "{}{}",
        "界".repeat(max_bytes / 3),
        "x".repeat(max_bytes % 3)
    );
    assert_eq!(boundary.len(), max_bytes);
    let mut value = json!({"request":{"action":"append","transactionId":"current-transaction","index":7,"expectedDraftRevision":9,"content":boundary}});
    assert!(inspect(&value).is_empty());
    value["request"]["content"] = json!(format!(
        "{}界",
        value["request"]["content"].as_str().unwrap()
    ));
    let details = diagnostics(&value);
    let issue = &details["diagnostics"][0];
    assert_eq!(issue["pointer"], "/request/content");
    assert_eq!(issue["reason"], "utf8_byte_limit");
    assert_eq!(issue["actualBytes"], max_bytes + 3);
    assert_eq!(issue["maxBytes"], max_bytes);
    let message = issue["message"].as_str().unwrap();
    assert!(message.contains("was not executed"));
    assert!(message.contains("smaller non-empty UTF-8 chunks"));
    assert!(message.contains("Keep the current transaction"));
    assert!(message.contains("latest nextIndex/draftRevision"));
    assert_eq!(details["recovery"], "correct_arguments");
    assert_eq!(details["outcome"], "definitely_not_executed");
    assert!(details["continueWith"].is_null());
    assert!(!details.to_string().contains('界'));
}

#[test]
fn malformed_values_are_diagnosed_without_panicking_or_echoing_them() {
    for request in [
        json!({"operation":"update"}),
        json!({"action":"PRIVATE_UNKNOWN_ACTION","operation":"update"}),
        json!({"action":null,"operation":"update"}),
        json!({"action":42,"operation":"update"}),
        json!({"action":"begin","operation":"delete"}),
        json!({"action":"apply","operation":"update","edits":[{"kind":"PRIVATE_UNKNOWN_KIND"}]}),
    ] {
        let value = json!({"request":request});
        let details = diagnostics(&value);
        assert!(!details.to_string().contains("PRIVATE_"));
        assert!(details["diagnostics"]
            .as_array()
            .is_some_and(|items| !items.is_empty()));
    }
}

#[test]
fn scalar_boundaries_and_both_update_payloads_are_closed() {
    let mut value = json!({"request":{"action":"append","transactionId":"tx","index":u64::MAX,"expectedDraftRevision":u64::MAX,"content":"x"}});
    assert!(inspect(&value).is_empty());
    for invalid in [
        json!(-1),
        json!(1.5),
        json!(null),
        json!("0"),
        json!(18446744073709551616.0),
    ] {
        value["request"]["index"] = invalid;
        assert!(!inspect(&value).is_empty());
    }
    for field in ["filePath", "observationId"] {
        let mut value = valid_update();
        value["request"][field] = json!("  \t");
        assert!(!inspect(&value).is_empty());
    }
    let mut value = valid_update();
    value["request"]["content"] = json!("x");
    assert!(inspect(&value)
        .iter()
        .any(|issue| issue.details["reason"] == "exclusive_fields"));
}

#[test]
fn diagnostics_are_bounded_and_bad_authority_cannot_panic_recovery() {
    let mut value = valid_update();
    value["request"]["content"] = json!("x".repeat(MAX_INLINE_CONTENT_BYTES + 1));
    value["request"].as_object_mut().unwrap().remove("edits");
    value["request"]["observationId"] = json!("PRIVATE_AUTHORITY".repeat(200));
    let details = diagnostics(&value);
    assert_eq!(details["continueWith"]["tool"], "read_file");
    assert!(!details.to_string().contains("PRIVATE_AUTHORITY"));
    let mut object = Map::new();
    for index in 0..100 {
        object.insert(
            format!("{index}_{}", "PRIVATE_KEY".repeat(100)),
            json!("PRIVATE_BODY"),
        );
    }
    let details = diagnostics(&Value::Object(object));
    assert_eq!(details["diagnostics"].as_array().unwrap().len(), MAX_ISSUES);
    assert!(!details.to_string().contains("PRIVATE_"));
    assert!(details.to_string().len() < 8_000);
}
