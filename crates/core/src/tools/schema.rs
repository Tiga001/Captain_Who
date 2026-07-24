use crate::protocol::{AgentError, AgentResult};
use serde_json::{json, Value};
use std::collections::BTreeSet;

const FORBIDDEN_ROOT_KEYWORDS: [&str; 6] = ["oneOf", "anyOf", "allOf", "enum", "const", "not"];

/// One provider-portable, model-facing schema for every tool that accepts an
/// [`crate::protocol::AgentFileInputRef`].
///
/// Keeping the discriminated branches in one place prevents a weak model from seeing different
/// attachment, workspace, Artifact, or Skill-resource contracts depending on which consuming tool
/// it happens to call.
pub(crate) fn agent_file_input_ref_schema() -> Value {
    json!({
        "description": "Unified AgentFileInputRef. Select exactly one source type and fill only that branch. The backend resolves authority, verifies immutable receipts where applicable, and reads exact bytes without exposing private storage paths.",
        "oneOf": [
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "type": {
                        "type": "string",
                        "enum": ["attachment"],
                        "description": "Read an exact attachment returned by the attachment library."
                    },
                    "readPath": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 32768,
                        "description": "Exact @attachments/... readPath returned by attachments_list."
                    }
                },
                "required": ["type", "readPath"]
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "type": {
                        "type": "string",
                        "enum": ["workspace"],
                        "description": "Read a file inside the selected workspace."
                    },
                    "path": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 32768,
                        "description": "Workspace-relative path. Do not provide an absolute path in this branch."
                    }
                },
                "required": ["type", "path"]
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "type": {
                        "type": "string",
                        "enum": ["external"],
                        "description": "Read an absolute path or supported system-path alias."
                    },
                    "path": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 32768,
                        "description": "Absolute path or supported system alias. A path that resolves inside the selected workspace is treated as workspace-scoped; a genuinely external file requires read=all."
                    }
                },
                "required": ["type", "path"]
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "type": {
                        "type": "string",
                        "enum": ["generated_artifact"],
                        "description": "Read an immutable application-published generated Artifact."
                    },
                    "uri": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 32768,
                        "description": "Exact content-addressed Artifact URI returned by the generating tool."
                    },
                    "path": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 32768,
                        "description": "Exact savedPath returned with the Artifact URI. This is a consistency hint, not an independent filesystem grant."
                    }
                },
                "required": ["type", "uri", "path"]
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "type": {
                        "type": "string",
                        "enum": ["skill_resource"],
                        "description": "Read an immutable resource from an activated Skill."
                    },
                    "uri": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 32768,
                        "description": "Exact revision-bound skill:// URI returned by the Skill resource tools."
                    }
                },
                "required": ["type", "uri"]
            }
        ]
    })
}

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
