use crate::protocol::{AgentError, AgentResult};
use serde_json::Value;
use std::collections::BTreeSet;

const FORBIDDEN_ROOT_KEYWORDS: [&str; 6] = ["oneOf", "anyOf", "allOf", "enum", "const", "not"];

/// Validates the portable root contract shared by the model providers we support.
/// Richer JSON Schema constraints remain available below the root object.
pub(crate) fn validate_portable_tool_input_schema(
    tool_name: &str,
    schema: &Value,
) -> AgentResult<()> {
    let root = schema.as_object().ok_or_else(|| {
        schema_error(
            tool_name,
            "根节点必须是 JSON object，并声明 `type: \"object\"`。",
        )
    })?;

    if root.get("type").and_then(Value::as_str) != Some("object") {
        return Err(schema_error(
            tool_name,
            "根节点必须明确声明 `type: \"object\"`。",
        ));
    }

    if let Some(keyword) = FORBIDDEN_ROOT_KEYWORDS
        .iter()
        .find(|keyword| root.contains_key(**keyword))
    {
        return Err(schema_error(
            tool_name,
            &format!(
                "根节点不能使用 `{keyword}`；请用 object properties 和 required 表达工具参数。"
            ),
        ));
    }

    let properties = match root.get("properties") {
        Some(Value::Object(properties)) => Some(properties),
        Some(_) => {
            return Err(schema_error(
                tool_name,
                "根节点的 `properties` 必须是 JSON object。",
            ));
        }
        None => None,
    };

    let Some(required) = root.get("required") else {
        return Ok(());
    };
    let required = required
        .as_array()
        .ok_or_else(|| schema_error(tool_name, "根节点的 `required` 必须是由参数名组成的数组。"))?;
    let mut seen = BTreeSet::new();
    for name in required {
        let name = name
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| schema_error(tool_name, "根节点的 `required` 只能包含非空字符串。"))?;
        if !seen.insert(name) {
            return Err(schema_error(
                tool_name,
                &format!("根节点的 `required` 重复声明了 `{name}`。"),
            ));
        }
        if !properties.is_some_and(|properties| properties.contains_key(name)) {
            return Err(schema_error(
                tool_name,
                &format!("根节点的 `required` 引用了未在 `properties` 中声明的 `{name}`。"),
            ));
        }
    }

    Ok(())
}

fn schema_error(tool_name: &str, detail: &str) -> AgentError {
    AgentError::new(format!(
        "工具 `{tool_name}` 的 input_schema 不兼容当前模型工具协议：{detail}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_portable_object_schema_with_nested_constraints() {
        let schema = json!({
            "type": "object",
            "properties": {
                "mode": { "type": "string", "enum": ["read", "write"] },
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "value": { "type": "string" }
                        },
                        "required": ["value"]
                    }
                }
            },
            "required": ["mode"]
        });

        validate_portable_tool_input_schema("portable", &schema).unwrap();
    }

    #[test]
    fn rejects_provider_incompatible_root_composition() {
        let error = validate_portable_tool_input_schema(
            "read_file",
            &json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "anyOf": [{ "required": ["path"] }]
            }),
        )
        .unwrap_err();

        assert!(error.to_string().contains("read_file"));
        assert!(error.to_string().contains("anyOf"));
    }

    #[test]
    fn rejects_required_property_that_is_not_declared() {
        let error = validate_portable_tool_input_schema(
            "broken",
            &json!({
                "type": "object",
                "properties": {},
                "required": ["missing"]
            }),
        )
        .unwrap_err();

        assert!(error.to_string().contains("missing"));
    }
}
