//! The apply_patch wire contract is the published schema. These checks cover its two object
//! levels (request and edits), not arbitrary JSON Schema. No caller value is used as a schema.
use super::*;

const MAX_ISSUES: usize = 16;

#[derive(Debug)]
pub(super) struct WireIssue {
    pub(super) code: FileChangeErrorCode,
    pub(super) details: Value,
}

pub(super) fn inspect(value: &Value) -> Vec<WireIssue> {
    let schema = schema::contract_schema();
    let mut issues = Vec::new();
    inspect_object(value, &[schema], "", &mut issues);
    issues
}

fn properties(schema: &Value) -> &Map<String, Value> {
    schema["properties"]
        .as_object()
        .expect("closed object schema")
}

fn required(schema: &Value) -> Vec<&str> {
    schema["required"]
        .as_array()
        .expect("object required fields")
        .iter()
        .map(|field| field.as_str().expect("required field name"))
        .collect()
}

fn branches(schema: &Value) -> Vec<&Value> {
    schema["oneOf"]
        .as_array()
        .map(|branches| branches.iter().collect())
        .unwrap_or_else(|| vec![schema])
}

fn enum_values(fields: &[&Value]) -> Vec<Value> {
    let mut allowed = Vec::new();
    for field in fields {
        for option in field["enum"].as_array().into_iter().flatten() {
            if !allowed.contains(option) {
                allowed.push(option.clone());
            }
        }
    }
    allowed
}

// Selection is only for diagnostics. Missing fields are still rejected, and input is never
// repaired. Positive payload evidence can select update, but absence must never suggest delete.
fn relevant_branches<'a>(all: &[&'a Value], object: &Map<String, Value>) -> Vec<&'a Value> {
    let mut selected = all.to_vec();
    for key in ["action", "operation", "kind"] {
        if let Some(value) = object.get(key).filter(|value| value.is_string()) {
            let matching: Vec<_> = selected
                .iter()
                .copied()
                .filter(|schema| {
                    schema["properties"][key]["enum"]
                        .as_array()
                        .is_some_and(|values| values.contains(value))
                })
                .collect();
            if !matching.is_empty() {
                selected = matching;
            }
        }
    }
    for key in ["edits", "content", "strategy"] {
        if object.contains_key(key) {
            // Keep both update alternatives when both mutually exclusive payloads are present.
            if matches!(key, "content" | "edits")
                && object.contains_key("content")
                && object.contains_key("edits")
            {
                continue;
            }
            let matching: Vec<_> = selected
                .iter()
                .copied()
                .filter(|schema| properties(schema).contains_key(key))
                .collect();
            if !matching.is_empty() {
                selected = matching;
            }
        }
    }
    selected
}

fn issue(
    issues: &mut Vec<WireIssue>,
    code: FileChangeErrorCode,
    pointer: &str,
    reason: &str,
    message: &str,
    extra: Value,
) {
    if issues.len() >= MAX_ISSUES {
        return;
    }
    let mut details = json!({"pointer":pointer,"reason":reason,"message":message});
    if let Some(extra) = extra.as_object() {
        details.as_object_mut().unwrap().extend(extra.clone());
    }
    issues.push(WireIssue { code, details });
}

fn child_pointer(parent: &str, key: &str) -> String {
    // Only echo short field identifiers. A caller can put a document, URL or token in a key.
    if key.len() > 64
        || !key.starts_with(|ch: char| ch.is_ascii_alphabetic() || ch == '_')
        || !key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    {
        return parent.to_string();
    }
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn inspect_object(value: &Value, all: &[&Value], pointer: &str, issues: &mut Vec<WireIssue>) {
    let Some(object) = value.as_object() else {
        issue(
            issues,
            FileChangeErrorCode::InvalidArguments,
            pointer,
            "type",
            "Expected an object.",
            json!({"expectedType":"object","actualType":type_name(value)}),
        );
        return;
    };
    let selected = relevant_branches(all, object);
    for key in object.keys() {
        if issues.len() >= MAX_ISSUES {
            break;
        }
        let known = all
            .iter()
            .any(|schema| properties(schema).contains_key(key));
        let allowed = selected
            .iter()
            .any(|schema| properties(schema).contains_key(key));
        if !allowed {
            let location = child_pointer(pointer, key);
            let message = if pointer.is_empty()
                && branches(&schema::contract_schema()["properties"]["request"])
                    .iter()
                    .any(|branch| properties(branch).contains_key(key))
            {
                "This field belongs inside /request; only request is allowed at the root."
            } else if known {
                "This field is not allowed in the selected branch; remove it or explicitly choose the intended branch."
            } else {
                "Unknown field. Use the exact field names in expectedShape."
            };
            issue(
                issues,
                if known {
                    FileChangeErrorCode::IllegalFieldCombination
                } else {
                    FileChangeErrorCode::UnknownField
                },
                &location,
                if known {
                    "illegal_field"
                } else {
                    "unknown_field"
                },
                message,
                Value::Null,
            );
        }
    }
    for key in required(selected[0]) {
        if selected
            .iter()
            .all(|schema| required(schema).contains(&key))
            && !object.contains_key(key)
        {
            let message = match key {
                "filePath" => "Missing filePath. For update/delete, copy fileChangeTarget.filePath from read_file or the latest successful write.",
                "observationId" => "Missing observationId. Copy the same target's fileChangeTarget.observationId; never invent it. Read the file if no current observation is available.",
                "transactionId" => "Missing transactionId. Copy the exact Host transactionId from begin or status.",
                "index" => "Missing index. Copy nextIndex from the latest successful mutation or status.",
                "expectedDraftRevision" => "Missing expectedDraftRevision. Copy draftRevision from the latest successful mutation or status.",
                _ => "Required field is missing. Supply it explicitly using expectedShape.",
            };
            let code = if pointer.is_empty() || matches!(key, "action" | "kind") {
                FileChangeErrorCode::InvalidArguments
            } else {
                FileChangeErrorCode::IllegalFieldCombination
            };
            let allowed = enum_values(
                &selected
                    .iter()
                    .filter_map(|schema| properties(schema).get(key))
                    .collect::<Vec<_>>(),
            );
            issue(
                issues,
                code,
                &child_pointer(pointer, key),
                "missing_field",
                message,
                if allowed.is_empty() {
                    Value::Null
                } else {
                    json!({"allowedValues":allowed})
                },
            );
        }
    }
    if selected.len() > 1
        && selected.iter().all(|schema| {
            schema["properties"]["action"]["enum"] == json!(["apply"])
                && schema["properties"]["operation"]["enum"] == json!(["update"])
        })
        && selected
            .iter()
            .any(|schema| properties(schema).contains_key("content"))
        && selected
            .iter()
            .any(|schema| properties(schema).contains_key("edits"))
    {
        issue(
            issues,
            FileChangeErrorCode::IllegalFieldCombination,
            pointer,
            "exclusive_fields",
            "An apply/update request requires exactly one of content or edits.",
            json!({"exactlyOneOf":["content","edits"]}),
        );
    }
    for (key, value) in object {
        if issues.len() >= MAX_ISSUES {
            break;
        }
        let fields: Vec<_> = selected
            .iter()
            .filter_map(|schema| properties(schema).get(key))
            .collect();
        if fields.is_empty() {
            continue;
        }
        let location = child_pointer(pointer, key);
        match key.as_str() {
            "request" if pointer.is_empty() => {
                inspect_object(value, &branches(fields[0]), &location, issues)
            }
            "edits" if pointer == "/request" => {
                if let Some(edits) = value.as_array() {
                    let min = fields[0]["minItems"].as_u64().unwrap() as usize;
                    let max = fields[0]["maxItems"].as_u64().unwrap() as usize;
                    if edits.len() < min || edits.len() > max {
                        issue(
                            issues,
                            FileChangeErrorCode::InvalidArguments,
                            &location,
                            "item_count",
                            "Edit count is outside the allowed range.",
                            json!({"actualItems":edits.len(),"minItems":min,"maxItems":max}),
                        );
                    }
                    let edit_branches = branches(&fields[0]["items"]);
                    for (index, edit) in edits.iter().take(max).enumerate() {
                        if issues.len() >= MAX_ISSUES {
                            break;
                        }
                        inspect_object(
                            edit,
                            &edit_branches,
                            &format!("{location}/{index}"),
                            issues,
                        );
                    }
                } else {
                    issue(
                        issues,
                        FileChangeErrorCode::InvalidArguments,
                        &location,
                        "type",
                        "Expected an array of edits.",
                        json!({"expectedType":"array","actualType":type_name(value)}),
                    );
                }
            }
            _ => inspect_scalar(
                value,
                &fields,
                &location,
                key,
                object.get("action").and_then(Value::as_str),
                issues,
            ),
        }
    }
}

fn inspect_scalar(
    value: &Value,
    fields: &[&Value],
    pointer: &str,
    key: &str,
    action: Option<&str>,
    issues: &mut Vec<WireIssue>,
) {
    let field = fields[0];
    let expected_type = field["type"].as_str().expect("scalar type");
    let valid_type = match expected_type {
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.as_u64().is_some(),
        _ => unreachable!("apply_patch has only string, boolean and nonnegative u64 scalar fields"),
    };
    if !valid_type {
        issue(
            issues,
            FileChangeErrorCode::InvalidArguments,
            pointer,
            "type",
            if expected_type == "integer" {
                "Expected a nonnegative integer representable as u64."
            } else {
                "Value has the wrong type; null is not accepted."
            },
            json!({"expectedType":expected_type,"actualType":type_name(value)}),
        );
        return;
    }
    let allowed = enum_values(fields);
    if !allowed.is_empty() && !allowed.contains(value) {
        issue(
            issues,
            FileChangeErrorCode::IllegalFieldCombination,
            pointer,
            "enum",
            "Choose one of the allowed literal values.",
            json!({"allowedValues":allowed}),
        );
    }
    if let Some(text) = value.as_str() {
        let count = text.chars().count();
        if matches!(key, "filePath" | "observationId" | "transactionId")
            && !text.is_empty()
            && text.trim().is_empty()
        {
            issue(
                issues,
                FileChangeErrorCode::InvalidArguments,
                pointer,
                "blank_string",
                "A file path or Host-issued identifier must not be blank.",
                Value::Null,
            );
        }
        if field["minLength"]
            .as_u64()
            .is_some_and(|min| count < min as usize)
        {
            issue(
                issues,
                FileChangeErrorCode::InvalidArguments,
                pointer,
                "min_length",
                "This string must not be empty.",
                json!({"minCharacters":field["minLength"],"actualCharacters":count}),
            );
        }
        if let Some(max) = field["maxLength"].as_u64() {
            // content's published character cap is a necessary condition for its UTF-8 byte cap.
            // Use that same limit here instead of maintaining a second action/limit table.
            if key == "content" && text.len() > max as usize {
                let message = if action == Some("append") {
                    "This append was not executed. Split the content into smaller non-empty UTF-8 chunks. Keep the current transaction and copy its latest nextIndex/draftRevision; use status if those cursors are uncertain. Do not begin a new transaction."
                } else {
                    "Content exceeds the UTF-8 byte limit. Character count is not byte count. Use the begin continuation to assemble larger Direct content in a staged transaction."
                };
                issue(
                    issues,
                    FileChangeErrorCode::ContentTooLarge,
                    pointer,
                    "utf8_byte_limit",
                    message,
                    json!({"actualBytes":text.len(),"maxBytes":max,"actualCharacters":count}),
                );
            } else if count > max as usize {
                issue(
                    issues,
                    FileChangeErrorCode::InvalidArguments,
                    pointer,
                    "max_length",
                    "String exceeds the character limit.",
                    json!({"actualCharacters":count,"maxCharacters":max}),
                );
            }
        }
    }
}

fn shape(schema: &Value) -> Value {
    let required = required(schema);
    let allowed: Vec<_> = properties(schema).keys().collect();
    let optional: Vec<_> = allowed
        .iter()
        .filter(|key| !required.contains(&key.as_str()))
        .collect();
    json!({"requiredFields":required,"optionalFields":optional,"allowedFields":allowed})
}

pub(super) fn expected_shape(value: &Value) -> Value {
    let schema = schema::contract_schema();
    let all = branches(&schema["properties"]["request"]);
    let mut result = json!({"root":shape(schema)});
    let Some(request) = apply_patch_request(value) else {
        result["request"] = branch_choices(&all);
        return result;
    };
    let selected = relevant_branches(&all, request);
    if selected.len() != 1 {
        result["request"] = branch_choices(&selected);
        return result;
    }
    let branch = selected[0];
    let mut request_shape = shape(branch);
    for key in ["action", "operation"] {
        if let Some(value) = properties(branch)
            .get(key)
            .and_then(|field| field["enum"].as_array())
            .and_then(|values| values.first())
        {
            request_shape[key] = value.clone();
        }
    }
    if let Some(content) = properties(branch).get("content") {
        request_shape["contentLimit"] = json!({"maxBytes":content["maxLength"],"encoding":"UTF-8","schemaMaxLengthCounts":"characters"});
    }
    if let Some(edits_schema) = properties(branch).get("edits") {
        // Only include the edit kinds the caller actually attempted, never all five shapes.
        let mut edit_shapes = Map::new();
        for edit in request
            .get("edits")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(128)
        {
            let Some(kind) = edit.get("kind").and_then(Value::as_str) else {
                continue;
            };
            for branch in branches(&edits_schema["items"]) {
                if properties(branch)["kind"]["enum"] == json!([kind]) {
                    edit_shapes.insert(kind.to_string(), shape(branch));
                }
            }
        }
        if !edit_shapes.is_empty() {
            request_shape["editShapes"] = Value::Object(edit_shapes);
        }
    }
    result["request"] = request_shape;
    result
}

fn branch_choices(branches: &[&Value]) -> Value {
    let mut actions = Vec::new();
    let mut operations = Vec::new();
    for branch in branches {
        for (key, values) in [("action", &mut actions), ("operation", &mut operations)] {
            if let Some(options) = properties(branch)
                .get(key)
                .and_then(|field| field["enum"].as_array())
            {
                for option in options {
                    if !values.contains(option) {
                        values.push(option.clone());
                    }
                }
            }
        }
    }
    json!({"allowedActions":actions,"allowedOperations":operations,"instruction":"Choose action and operation explicitly from the intended change; missing content does not imply delete."})
}

#[cfg(test)]
mod tests;
