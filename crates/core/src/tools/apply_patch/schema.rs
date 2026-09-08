use super::*;

pub(super) fn patch_input_schema() -> Value {
    let create_file_path = json!({
        "type": "string",
        "minLength": 1,
        "description": "New target path: workspace-relative, an authorized absolute path or system alias. No prior read required."
    });
    let observed_file_path = json!({
        "type": "string",
        "minLength": 1,
        "description": "Copy this target's exact fileChangeTarget.filePath from read_file or the latest successful apply/commit."
    });
    let observation_id = json!({
        "type": "string",
        "minLength": 1,
        "description": "Copy this target's exact fileChangeTarget.observationId from read_file or the latest successful apply/commit."
    });
    let transaction_id = json!({
        "type": "string",
        "minLength": 1,
        "description": "Exact Host transactionId from begin or status."
    });
    let index = json!({
        "type": "integer",
        "minimum": 0,
        "description": "Copy nextIndex from the latest successful mutation or status."
    });
    let draft_revision = json!({
        "type": "integer",
        "minimum": 0,
        "description": "Copy draftRevision from the latest successful mutation or status."
    });
    let summary = json!({
        "type": "string",
        "maxLength": MAX_SUMMARY_CHARS,
        "description": "Short human-readable change summary."
    });
    let edits = structured_edits_schema();
    json!({
        "type": "object",
        "properties": {
            "request": {
                "description": "Exactly one branch; no fields from other branches.",
                "oneOf": [
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["apply"] },
                            "operation": { "type": "string", "enum": ["create"] },
                            "filePath": create_file_path.clone(),
                            "content": {
                                "type": "string",
                                "maxLength": MAX_INLINE_CONTENT_BYTES,
                                "description": "Complete Direct file content, at most 32 KiB of UTF-8 bytes. Empty content creates an empty file."
                            },
                            "summary": summary.clone()
                        },
                        "required": ["action", "operation", "filePath", "content"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["apply"] },
                            "operation": { "type": "string", "enum": ["update"] },
                            "filePath": observed_file_path.clone(),
                            "observationId": observation_id.clone(),
                            "content": {
                                "type": "string",
                                "maxLength": MAX_INLINE_CONTENT_BYTES,
                                "description": "Complete replacement content, at most 32 KiB of UTF-8 bytes. For larger complete replacements use begin/update with strategy=rewrite."
                            },
                            "summary": summary.clone()
                        },
                        "required": ["action", "operation", "filePath", "observationId", "content"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["apply"] },
                            "operation": { "type": "string", "enum": ["update"] },
                            "filePath": observed_file_path.clone(),
                            "observationId": observation_id.clone(),
                            "edits": edits.clone(),
                            "summary": summary.clone()
                        },
                        "required": ["action", "operation", "filePath", "observationId", "edits"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["apply"] },
                            "operation": { "type": "string", "enum": ["delete"] },
                            "filePath": observed_file_path.clone(),
                            "observationId": observation_id.clone(),
                            "summary": summary.clone()
                        },
                        "required": ["action", "operation", "filePath", "observationId"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["begin"] },
                            "operation": { "type": "string", "enum": ["create"] },
                            "filePath": create_file_path,
                        },
                        "required": ["action", "operation", "filePath"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["begin"] },
                            "operation": { "type": "string", "enum": ["update"] },
                            "filePath": observed_file_path,
                            "observationId": observation_id.clone(),
                            "strategy": {
                                "type": "string",
                                "enum": ["modify", "rewrite"],
                                "description": "modify starts from observed content; rewrite starts from an empty draft."
                            }
                        },
                        "required": ["action", "operation", "filePath", "observationId", "strategy"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["append"] },
                            "transactionId": transaction_id.clone(),
                            "index": index.clone(),
                            "expectedDraftRevision": draft_revision.clone(),
                            "content": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": file_change_staged::MAX_STAGED_CHUNK_BYTES,
                                "description": "One non-empty append chunk, at most 1 MiB of UTF-8 bytes. The complete transaction may not exceed 4 MiB."
                            }
                        },
                        "required": ["action", "transactionId", "index", "expectedDraftRevision", "content"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["edit"] },
                            "transactionId": transaction_id.clone(),
                            "index": index,
                            "expectedDraftRevision": draft_revision.clone(),
                            "edits": edits
                        },
                        "required": ["action", "transactionId", "index", "expectedDraftRevision", "edits"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["commit"] },
                            "transactionId": transaction_id.clone(),
                            "expectedDraftRevision": draft_revision,
                            "summary": summary
                        },
                        "required": ["action", "transactionId", "expectedDraftRevision"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["status"] },
                            "transactionId": transaction_id.clone()
                        },
                        "required": ["action", "transactionId"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["abort"] },
                            "transactionId": transaction_id
                        },
                        "required": ["action", "transactionId"],
                        "additionalProperties": false
                    }
                ]
            }
        },
        "required": ["request"],
        "additionalProperties": false
    })
}

fn structured_edits_schema() -> Value {
    json!({
        "type": "array",
        "minItems": 1,
        "maxItems": 128,
        "description": "Apply 1-128 edits in order using exact bytes: no trimming, normalization or fuzzy matching. replace requires one match unless replaceAll=true. Final size limit: Direct 240,000 UTF-8 bytes; Staged 4 MiB.",
        "items": {
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["replace"] },
                        "oldText": { "type": "string", "minLength": 1 },
                        "newText": { "type": "string" },
                        "replaceAll": { "type": "boolean" }
                    },
                    "required": ["kind", "oldText", "newText"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["insert_before"] },
                        "anchor": { "type": "string", "minLength": 1 },
                        "text": { "type": "string" }
                    },
                    "required": ["kind", "anchor", "text"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["insert_after"] },
                        "anchor": { "type": "string", "minLength": 1 },
                        "text": { "type": "string" }
                    },
                    "required": ["kind", "anchor", "text"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["append"] },
                        "text": { "type": "string" }
                    },
                    "required": ["kind", "text"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["prepend"] },
                        "text": { "type": "string" }
                    },
                    "required": ["kind", "text"],
                    "additionalProperties": false
                }
            ]
        }
    })
}
