use crate::conversation_trace::canonical_tool_result_for_context;
use crate::protocol::AgentToolResult;
use serde_json::{Map, Value};

pub(crate) fn compact_model_result(
    result: &AgentToolResult,
    projected_result: Option<Value>,
) -> AgentToolResult {
    let projected_result = if result.ok {
        projected_result
    } else {
        merge_failure_metadata(result.result.as_ref(), projected_result)
    };
    let mut projected = AgentToolResult {
        exact_archive_file: None,
        call_id: result.call_id.clone(),
        tool: result.tool.clone(),
        ok: result.ok,
        result: projected_result.and_then(prune_model_value),
        error: result
            .error
            .as_deref()
            .map(str::trim)
            .filter(|error| !error.is_empty())
            .map(ToString::to_string),
    };
    remove_duplicate_error_fields(&mut projected);
    canonical_tool_result_for_context(&projected)
}

fn merge_failure_metadata(source: Option<&Value>, projected: Option<Value>) -> Option<Value> {
    let Some(source) = source.and_then(Value::as_object) else {
        return projected;
    };
    let mut output = match projected {
        Some(Value::Object(output)) => output,
        Some(value) => {
            let mut output = Map::new();
            output.insert("detail".to_string(), value);
            output
        }
        None => Map::new(),
    };
    for field in [
        "type",
        "code",
        "errorCode",
        "status",
        "recovery",
        "phase",
        "executionAttempted",
        "effectsMayHaveOccurred",
        "commitMayHaveSucceeded",
        "generationMayHaveSucceeded",
        "providerSucceeded",
        "artifactCommitMayHaveSucceeded",
        "retryable",
        "requiredCapability",
        "bypassAllowed",
        "message",
    ] {
        if output.contains_key(field) {
            continue;
        }
        if let Some(value) = source.get(field).cloned().and_then(prune_model_value) {
            output.insert(field.to_string(), value);
        }
    }
    (!output.is_empty()).then_some(Value::Object(output))
}

pub(super) fn retain_fields(value: Option<&Value>, fields: &[&str]) -> Option<Value> {
    let source = value?.as_object()?;
    let mut projected = Map::new();
    for field in fields {
        if let Some(value) = source.get(*field).cloned().and_then(prune_model_value) {
            projected.insert((*field).to_string(), value);
        }
    }
    (!projected.is_empty()).then_some(Value::Object(projected))
}

pub(crate) fn retain_object_fields(value: &Value, fields: &[&str]) -> Option<Value> {
    retain_fields(Some(value), fields)
}

pub(crate) fn insert_field(target: &mut Map<String, Value>, source: &Value, field: &str) {
    if let Some(value) = source.get(field).cloned().and_then(prune_model_value) {
        target.insert(field.to_string(), value);
    }
}

pub(super) fn prune_model_value(value: Value) -> Option<Value> {
    match value {
        Value::Null => None,
        Value::String(value) => {
            let value = value.trim();
            (!value.is_empty()).then(|| Value::String(value.to_string()))
        }
        Value::Array(values) => {
            let values = values
                .into_iter()
                .filter_map(prune_model_value)
                .collect::<Vec<_>>();
            (!values.is_empty()).then_some(Value::Array(values))
        }
        Value::Object(values) => {
            let values = values
                .into_iter()
                .filter_map(|(key, value)| prune_model_value(value).map(|value| (key, value)))
                .collect::<Map<_, _>>();
            (!values.is_empty()).then_some(Value::Object(values))
        }
        value => Some(value),
    }
}

fn remove_duplicate_error_fields(result: &mut AgentToolResult) {
    let Some(error) = result.error.as_deref() else {
        return;
    };
    let Some(value) = result.result.as_mut() else {
        return;
    };
    remove_duplicate_error_fields_from_value(value, error);
    if value.as_object().is_some_and(Map::is_empty) {
        result.result = None;
    }
}

fn remove_duplicate_error_fields_from_value(value: &mut Value, error: &str) {
    match value {
        Value::Object(object) => {
            for field in ["error", "message"] {
                if object
                    .get(field)
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.trim() == error)
                {
                    object.remove(field);
                }
            }
            for value in object.values_mut() {
                remove_duplicate_error_fields_from_value(value, error);
            }
        }
        Value::Array(values) => {
            for value in values {
                remove_duplicate_error_fields_from_value(value, error);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compact_projection_removes_empty_values_and_duplicate_errors() {
        let raw = AgentToolResult {
            exact_archive_file: None,
            call_id: "call-1".to_string(),
            tool: "test".to_string(),
            ok: false,
            result: Some(json!({
                "code": "failed",
                "message": "same error",
                "failure": { "message": "same error", "recovery": "retry" },
                "empty": "",
                "emptyArray": [],
                "emptyObject": {},
                "keptFalse": false,
                "keptZero": 0
            })),
            error: Some("same error".to_string()),
        };

        let projected = compact_model_result(&raw, raw.result.clone());
        let value = projected.result.unwrap();
        assert_eq!(value["code"], "failed");
        assert_eq!(value["keptFalse"], false);
        assert_eq!(value["keptZero"], 0);
        assert!(value.get("message").is_none());
        assert!(value["failure"].get("message").is_none());
        assert_eq!(value["failure"]["recovery"], "retry");
        assert!(value.get("empty").is_none());
        assert!(value.get("emptyArray").is_none());
        assert!(value.get("emptyObject").is_none());
    }
}
