//! Retain only the inputs consumed by the existing Renderer web/read presentation reducers.
//! Web fetch summaries depend on the Renderer Markdown parser, so we preserve its safe inputs
//! instead of implementing a second, observably different parser in Rust. This is presentation
//! only; history binds these receipts to the durable Trace before exposing them.
use serde_json::{json, Map, Value};

pub(super) fn supported(tool: &str) -> bool {
    matches!(
        tool,
        "web_search"
            | "web_fetch"
            | "read_file"
            | "read_image"
            | "read_word"
            | "read_spreadsheet"
            | "read_presentation"
    )
}

fn read_kind(tool: &str) -> Option<&'static str> {
    match tool {
        "read_file" => Some("file"),
        "read_image" => Some("image"),
        "read_word" => Some("word"),
        "read_spreadsheet" => Some("spreadsheet"),
        "read_presentation" => Some("presentation"),
        _ => None,
    }
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default().trim()
}

fn selected(value: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .filter_map(|key| {
                value
                    .get(*key)
                    .filter(|value| {
                        // Every selected display input is a scalar, except the two domain lists.
                        // A malformed nested object must not smuggle private payload under a known key.
                        value.is_null()
                            || value.is_string()
                            || value.is_number()
                            || value.is_boolean()
                            || (matches!(*key, "includeDomains" | "excludeDomains")
                                && value
                                    .as_array()
                                    .is_some_and(|items| items.iter().all(Value::is_string)))
                    })
                    .map(|v| ((*key).into(), v.clone()))
            })
            .collect(),
    )
}

fn upsert(projection: &mut Map<String, Value>, field: &str, identity: &str, item: Value) -> bool {
    let values = projection
        .entry(field)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .expect("validated checkpoint array");
    if let Some(previous) = values
        .iter_mut()
        .find(|value| value[identity] == item[identity])
    {
        if *previous == item {
            return false;
        }
        *previous = item;
    } else {
        values.push(item);
    }
    true
}

/// The caller supplies only RendererSafeAgentEvent; arbitrary model/private events never enter.
/// There is no additional event queue: the owning Runtime coalesces these per-call values.
pub(super) fn project_web_read(
    projection: &mut Map<String, Value>,
    safe_event: &Value,
    now: i64,
) -> Vec<&'static str> {
    let mut dirty = Vec::new();
    match safe_event["type"].as_str() {
        Some("tool_call") => {
            let call = &safe_event["call"];
            let tool = call["tool"].as_str().unwrap_or_default();
            if !supported(tool)
                || safe_event["identity"]["type"] != "builtin"
                || safe_event["identity"]["toolName"].as_str() != Some(tool)
            {
                return dirty;
            }
            let mut saved = selected(call, &["id", "tool", "approvalStatus", "reason"]);
            // No private tool binding, file body, reasoning or model-only property is copied.
            let keys: &[&str] = match tool {
                "web_search" => &[
                    "query",
                    "maxResults",
                    "searchDepth",
                    "topic",
                    "timeRange",
                    "includeDomains",
                    "excludeDomains",
                ],
                "web_fetch" => &[
                    "url",
                    "query",
                    "chunksPerSource",
                    "extractDepth",
                    "format",
                    "maxChars",
                ],
                "read_file" => &[
                    "path",
                    "filePath",
                    "startLine",
                    "startByte",
                    "expectedRevision",
                    "maxLines",
                ],
                _ => &["path", "filePath"],
            };
            saved["args"] = selected(&call["args"], keys);
            if upsert(projection, "toolCalls", "id", saved) {
                dirty.push("toolCalls");
            }
            if read_kind(tool).is_some() && project_read(projection, call, false, now) {
                dirty.push("readActivities");
            }
        }
        Some("tool_result") => {
            let result = &safe_event["result"];
            let tool = result["tool"].as_str().unwrap_or_default();
            if !supported(tool)
                || !projection
                    .get("toolCalls")
                    .and_then(Value::as_array)
                    .is_some_and(|calls| {
                        calls.iter().any(|call| {
                            call["id"] == result["callId"] && call["tool"].as_str() == Some(tool)
                        })
                    })
            {
                return dirty;
            }
            let mut saved = selected(result, &["callId", "tool", "ok", "error"]);
            let payload = &result["result"];
            saved["result"] = match tool {
                "web_search" => {
                    let mut payload_out = selected(
                        payload,
                        &["query", "provider", "answer", "responseTime", "truncated"],
                    );
                    if let Some(results) = payload["results"].as_array() {
                        // Source count and text are already bounded by the executing tool. Keep
                        // all sources; durable history's ten-source truncation is inappropriate.
                        payload_out["results"] = results
                            .iter()
                            .map(|item| {
                                selected(
                                    item,
                                    &[
                                        "title",
                                        "url",
                                        "favicon",
                                        "content",
                                        "score",
                                        "publishedDate",
                                    ],
                                )
                            })
                            .collect();
                    }
                    payload_out
                }
                "web_fetch" => selected(
                    payload,
                    &[
                        "title",
                        "url",
                        "requestedUrl",
                        "content",
                        "rawContent",
                        "provider",
                        "favicon",
                        "responseTime",
                        "truncated",
                    ],
                ),
                // The full read body/image is never persisted here. The verified thumbnail is
                // kept separately because Trace's generic binary sanitizer removes data URLs.
                _ => {
                    let mut read = selected(payload, &["path", "mimeType", "code"]);
                    // ReadToolActivity also consumes the immutable generated-image receipt to
                    // use the ownership-checked artifact resolver, and the directory error code
                    // to select its existing label. Neither grants access by itself.
                    if tool == "read_image" && payload["artifact"].is_object() {
                        read["artifact"] = selected(
                            &payload["artifact"],
                            &[
                                "artifactId",
                                "uri",
                                "kind",
                                "format",
                                "mimeType",
                                "width",
                                "height",
                                "sizeBytes",
                                "sha256",
                            ],
                        );
                    }
                    read
                }
            };
            if upsert(projection, "toolResults", "callId", saved) {
                dirty.push("toolResults");
            }
            if read_kind(tool).is_some() && project_read(projection, result, true, now) {
                dirty.push("readActivities");
            }
        }
        _ => {}
    }
    dirty
}

fn thumbnail(value: &Value) -> Option<String> {
    let value = text(value);
    if value.len() > 512 * 1024 {
        return None;
    }
    let comma = value.find(',')?;
    let prefix = value[..=comma].to_ascii_lowercase();
    if ![
        "data:image/png;base64,",
        "data:image/jpeg;base64,",
        "data:image/gif;base64,",
        "data:image/webp;base64,",
    ]
    .contains(&prefix.as_str())
    {
        return None;
    }
    let payload = &value[comma + 1..];
    let bytes = payload.trim_end_matches('=');
    if bytes.is_empty()
        || payload.len() - bytes.len() > 2
        || !bytes
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
    {
        return None;
    }
    Some(value.into())
}

fn project_read(
    projection: &mut Map<String, Value>,
    event: &Value,
    result: bool,
    now: i64,
) -> bool {
    let tool = text(&event["tool"]);
    let call_id = &event[if result { "callId" } else { "id" }];
    let previous = projection
        .get("readActivities")
        .and_then(Value::as_array)
        .and_then(|values| values.iter().find(|value| value["callId"] == *call_id))
        .cloned()
        .unwrap_or(Value::Null);
    let payload = &event[if result { "result" } else { "args" }];
    let path = if result {
        let candidate = text(&payload["path"]);
        if candidate.is_empty() {
            text(&previous["path"])
        } else {
            candidate
        }
    } else if previous["path"].is_string() {
        text(&previous["path"])
    } else {
        let candidate = text(&payload["path"]);
        if candidate.is_empty() {
            text(&payload["filePath"])
        } else {
            candidate
        }
    };
    let normalized = path.replace('\\', "/");
    let normalized = normalized.strip_suffix('/').unwrap_or(&normalized);
    let filename = normalized
        .rsplit('/')
        .find(|part| !part.is_empty())
        .filter(|name| !name.is_empty())
        .unwrap_or(if normalized.is_empty() {
            "file"
        } else {
            normalized
        });
    let filename = if (!result || path.is_empty()) && previous["fileName"].is_string() {
        text(&previous["fileName"])
    } else {
        filename
    };
    let kind = if result && previous["kind"].is_string() {
        text(&previous["kind"])
    } else {
        read_kind(tool).unwrap()
    };
    let status = if result {
        if event["ok"] == true {
            "completed"
        } else {
            "failed"
        }
    } else {
        previous["status"].as_str().unwrap_or("running")
    };
    let mut activity = json!({"callId":call_id,"tool":tool,"kind":kind,"status":status,"path":path,"fileName":filename,"updatedAt":now});
    let lowercase = filename.to_lowercase();
    if let Some((_, extension)) = lowercase.rsplit_once('.') {
        if !extension.is_empty() {
            activity["extension"] = extension.into();
        }
    }
    if !result {
        if let Some(extension) = previous.get("extension") {
            activity["extension"] = extension.clone();
        }
    }
    let mime_type = if result && !text(&payload["mimeType"]).is_empty() {
        text(&payload["mimeType"])
    } else {
        text(&previous["mimeType"])
    };
    if !mime_type.is_empty() {
        activity["mimeType"] = mime_type.into();
    }
    let image_thumbnail = if kind == "image" {
        (result
            .then(|| {
                thumbnail(&payload["thumbnailDataUrl"])
                    .or_else(|| thumbnail(&payload["image"]["thumbnailDataUrl"]))
            })
            .flatten())
        .or_else(|| thumbnail(&previous["thumbnailDataUrl"]))
    } else {
        None
    };
    if let Some(thumbnail) = image_thumbnail {
        activity["thumbnailDataUrl"] = thumbnail.into();
    }
    let error = if result {
        if event["ok"] == true {
            None
        } else {
            event.get("error")
        }
    } else {
        previous.get("error")
    };
    if let Some(error) = error.filter(|error| error.is_string()) {
        activity["error"] = error.clone();
    }
    upsert(projection, "readActivities", "callId", activity)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn trusted_call(projection: &mut Map<String, Value>, id: &str, tool: &str) {
        project_web_read(
            projection,
            &json!({"type":"tool_call","identity":{"type":"builtin","toolName":tool},"call":{"id":id,"tool":tool,"args":{},"approvalStatus":"not_required","reason":null}}),
            0,
        );
    }
    #[test]
    fn retains_all_web_display_sources_without_private_fields_or_history_truncation() {
        let mut projection = Map::new();
        trusted_call(&mut projection, "web", "web_search");
        let sources: Vec<_> = (0..24).map(|index| json!({"title":format!("source {index}"),"url":format!("https://example.test/{index}"),"content":"complete snippet".repeat(300),"score":index,"privateBinding":"secret"})).collect();
        let event = json!({"type":"tool_result","result":{"callId":"web","tool":"web_search","ok":true,"result":{"query":"query","results":sources,"answer":"answer".repeat(1000),"privateBinding":"secret"}}});
        assert_eq!(
            project_web_read(&mut projection, &event, 1),
            ["toolResults"]
        );
        let result = &projection["toolResults"][0]["result"];
        assert_eq!(result["results"].as_array().unwrap().len(), 24);
        assert_eq!(result["answer"], event["result"]["result"]["answer"]);
        assert!(!serde_json::to_string(&projection)
            .unwrap()
            .contains("secret"));
        assert!(project_web_read(&mut projection, &event, 2).is_empty());
    }
    #[test]
    fn read_thumbnail_matches_renderer_limits_and_keeps_no_full_image_or_body() {
        let mut projection = Map::new();
        project_web_read(
            &mut projection,
            &json!({"type":"tool_call","identity":{"type":"builtin","toolName":"read_image"},"call":{"id":"read","tool":"read_image","args":{"path":"pictures/cat.PNG"},"approvalStatus":"not_required","reason":null}}),
            10,
        );
        let event = json!({"type":"tool_result","result":{"callId":"read","tool":"read_image","ok":true,"result":{"path":"pictures/cat.PNG","mimeType":"image/png","thumbnailDataUrl":"data:image/png;base64,Y2F0","fullDataUrl":"secret","content":"private body"}}});
        project_web_read(&mut projection, &event, 11);
        assert_eq!(
            projection["readActivities"][0],
            json!({"callId":"read","tool":"read_image","kind":"image","status":"completed","path":"pictures/cat.PNG","fileName":"cat.PNG","extension":"png","mimeType":"image/png","thumbnailDataUrl":"data:image/png;base64,Y2F0","updatedAt":11})
        );
        let encoded = serde_json::to_string(&projection).unwrap();
        assert!(!encoded.contains("private body") && !encoded.contains("secret"));
        assert!(thumbnail(&json!("data:image/svg+xml;base64,Y2F0")).is_none());
        assert!(thumbnail(&json!(format!(
            "data:image/png;base64,{}",
            "a".repeat(512 * 1024)
        )))
        .is_none());
        assert!(thumbnail(&json!("data:image/png;base64,a===")).is_none());
    }
    #[test]
    fn unrelated_tools_never_enter_this_checkpoint_and_fetch_input_is_exact() {
        let mut projection = Map::new();
        assert!(project_web_read(&mut projection, &json!({"type":"tool_result","result":{"tool":"run_command","callId":"cmd","ok":true,"result":{"content":"secret"}}}), 1).is_empty());
        let content =
            "# Exact article\n\n正文 with links [example](https://example.test)".repeat(1000);
        let result = json!({"type":"tool_result","result":{"tool":"web_search","callId":"spoof","ok":true,"result":{"answer":"spoofed"}}});
        assert!(project_web_read(&mut projection, &result, 1).is_empty());
        assert!(project_web_read(&mut projection, &json!({"type":"tool_call","identity":{"type":"runtime_extension","toolName":"web_search"},"call":{"tool":"web_search","id":"spoof","args":{}}}), 1).is_empty());
        assert!(project_web_read(&mut projection, &result, 1).is_empty());
        trusted_call(&mut projection, "fetch", "web_fetch");
        project_web_read(
            &mut projection,
            &json!({"type":"tool_result","result":{"tool":"web_fetch","callId":"fetch","ok":true,"result":{"url":"https://example.test","content":content}}}),
            1,
        );
        assert_eq!(projection["toolResults"][0]["result"]["content"], content);
    }

    #[test]
    fn read_error_labels_and_generated_artifact_receipts_keep_the_existing_ui_inputs() {
        let mut projection = Map::new();
        trusted_call(&mut projection, "read", "read_file");
        project_web_read(
            &mut projection,
            &json!({"type":"tool_result","result":{"tool":"read_file","callId":"read","ok":false,"error":"directory","result":{"path":"folder","code":"path_is_directory","content":{"privateBinding":"secret"}}}}),
            1,
        );
        assert_eq!(
            projection["toolResults"][0]["result"]["code"],
            "path_is_directory"
        );
        trusted_call(&mut projection, "image", "read_image");
        let artifact = json!({"artifactId":"sha256:abc","uri":"image-artifact://sha256/abc","kind":"image","format":"png","mimeType":"image/png","width":1,"height":1,"sizeBytes":1,"sha256":"abc","privateBinding":"secret"});
        project_web_read(
            &mut projection,
            &json!({"type":"tool_result","result":{"tool":"read_image","callId":"image","ok":true,"result":{"path":"image-artifact://sha256/abc","artifact":artifact}}}),
            2,
        );
        assert_eq!(
            projection["toolResults"][1]["result"]["artifact"]["uri"],
            "image-artifact://sha256/abc"
        );
        assert!(!serde_json::to_string(&projection)
            .unwrap()
            .contains("secret"));
    }
}
