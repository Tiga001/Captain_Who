use super::{
    sanitize_limit, walk_workspace_with_cancellation, AgentTool, ToolExecutionContext, WalkEntry,
    WalkResult, MAX_SEARCH_FILE_BYTES, MAX_SEARCH_LIMIT, MAX_WALK_ENTRIES,
};
use crate::protocol::{
    AgentError, AgentResult, AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;

pub(super) struct SearchCodeTool;

impl AgentTool for SearchCodeTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "search_code".to_string(),
            description: "Search UTF-8 text content in an accessible file or directory. With no workspace, provide an absolute path or a system alias such as @desktop.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Text to search for." },
                    "path": { "type": "string", "description": "Optional workspace-relative or absolute directory/file, or @home/@desktop/@documents/@downloads. Defaults to workspace root when one exists." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_SEARCH_LIMIT },
                    "caseSensitive": { "type": "boolean" }
                },
                "required": ["query"]
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        let args: SearchCodeArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("search_code 参数无效：{error}")))?;
        let query = args.query.trim();
        if query.is_empty() {
            return Err(AgentError::new("search_code.query 不能为空。"));
        }

        let cancellation_token = context.cancellation_token();
        let search_root = match args.path.as_deref().filter(|path| !path.trim().is_empty()) {
            Some(path) => context.resolve_existing_path(path)?,
            None => context.workspace_root_optional()?.ok_or_else(|| {
                AgentError::new(
                    "当前没有 workspace；search_code.path 必须指定绝对目录、文件或系统路径别名。",
                )
            })?,
        };
        let limit = sanitize_limit(args.limit);
        let case_sensitive = args.case_sensitive.unwrap_or(false);
        let needle = if case_sensitive {
            query.to_string()
        } else {
            query.to_ascii_lowercase()
        };
        let mut matches = Vec::new();
        let walk = if search_root.is_file() {
            WalkResult {
                entries: vec![WalkEntry {
                    path: search_root.clone(),
                    is_dir: false,
                    size_bytes: fs::metadata(&search_root)
                        .map(|metadata| metadata.len())
                        .unwrap_or(0),
                }],
                truncated: false,
            }
        } else {
            walk_workspace_with_cancellation(&search_root, &cancellation_token)?
        };

        let mut skipped_too_large = 0usize;
        let mut read_failures = 0usize;
        for entry in walk.entries.iter().filter(|entry| !entry.is_dir) {
            cancellation_token.check()?;
            if entry.size_bytes > MAX_SEARCH_FILE_BYTES {
                skipped_too_large = skipped_too_large.saturating_add(1);
                continue;
            }

            let content = match fs::read_to_string(&entry.path) {
                Ok(content) => content,
                Err(_) => {
                    read_failures = read_failures.saturating_add(1);
                    continue;
                }
            };

            for (line_index, line) in content.lines().enumerate() {
                if line_index % 200 == 0 {
                    cancellation_token.check()?;
                }
                let haystack = if case_sensitive {
                    line.to_string()
                } else {
                    line.to_ascii_lowercase()
                };

                if !haystack.contains(&needle) {
                    continue;
                }

                matches.push(json!({
                    "path": context.display_path(
                        args.path.as_deref().unwrap_or("."),
                        &entry.path,
                    )?,
                    "lineNumber": line_index + 1,
                    "line": line.trim()
                }));

                if matches.len() >= limit {
                    break;
                }
            }

            if matches.len() >= limit {
                break;
            }
        }

        let mut output = json!({
            "query": query,
            "matches": matches,
            "truncated": walk.truncated || matches.len() >= limit
        });
        if let Some(omissions) = search_omissions(&walk, skipped_too_large, read_failures) {
            output["omissions"] = omissions;
        }
        if walk.truncated || skipped_too_large > 0 || read_failures > 0 {
            output["truncatedAtSource"] = Value::Bool(true);
        }

        Ok(output)
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        let projected = result.result.as_ref().map(|value| {
            let mut output = serde_json::Map::new();
            let matches = value
                .get("matches")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            output.insert(
                "matchCount".to_string(),
                Value::from(u64::try_from(matches.len()).unwrap_or(u64::MAX)),
            );
            if !matches.is_empty() {
                output.insert("matches".to_string(), Value::Array(matches));
            }
            super::model_projection::insert_field(&mut output, value, "truncated");
            super::model_projection::insert_field(&mut output, value, "omissions");
            Value::Object(output)
        });
        super::model_projection::compact_model_result(result, projected)
    }
}

fn search_omissions(
    walk: &WalkResult,
    skipped_too_large: usize,
    read_failures: usize,
) -> Option<Value> {
    if skipped_too_large == 0 && read_failures == 0 && !walk.truncated {
        return None;
    }

    let mut omissions = serde_json::Map::new();
    if skipped_too_large > 0 || read_failures > 0 {
        let mut by_reason = serde_json::Map::new();
        if skipped_too_large > 0 {
            by_reason.insert(
                "fileTooLarge".to_string(),
                json!({
                    "count": skipped_too_large,
                    "reason": "file_exceeds_search_size_limit",
                    "maxBytes": MAX_SEARCH_FILE_BYTES
                }),
            );
        }
        if read_failures > 0 {
            by_reason.insert(
                "readFailed".to_string(),
                json!({
                    "count": read_failures,
                    "reason": "file_could_not_be_read_as_utf8_text"
                }),
            );
        }
        omissions.insert(
            "files".to_string(),
            json!({
                "total": skipped_too_large.saturating_add(read_failures),
                "byReason": by_reason
            }),
        );
    }
    if walk.truncated {
        omissions.insert(
            "walk".to_string(),
            json!({
                "reason": "workspace_walk_entry_limit_reached",
                "returnedEntries": walk.entries.len(),
                "limitEntries": MAX_WALK_ENTRIES,
                "omittedCountKnown": false
            }),
        );
    }

    Some(Value::Object(omissions))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchCodeArgs {
    query: String,
    path: Option<String>,
    limit: Option<usize>,
    case_sensitive: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::super::{
        ToolExecutionContext, ToolRegistry, WalkEntry, WalkResult, MAX_SEARCH_FILE_BYTES,
        MAX_WALK_ENTRIES,
    };
    use super::search_omissions;
    use crate::protocol::{
        AgentApprovalStatus, AgentRunContext, AgentToolCall, AgentWorkspaceContext,
    };
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn search_code_finds_text_matches() {
        let fixture = TestWorkspace::new();
        fixture.write_file("src/lib.rs", "pub fn target_symbol() {}\n");
        let context = fixture.context();
        let registry = ToolRegistry::defaults_with_search(None);
        let call = AgentToolCall {
            id: "call-1".to_string(),
            tool: "search_code".to_string(),
            args: json!({ "query": "target_symbol" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let result = registry.execute(&context, &call);

        assert!(result.ok, "{:?}", result.error);
        assert!(!crate::tools::tool_result_truncated_at_source(&result));
        assert_eq!(
            result.result.as_ref().unwrap()["matches"][0]["lineNumber"],
            1
        );
        let model = registry.model_projection(&result);
        assert_eq!(model.result.as_ref().unwrap()["matchCount"], 1);
        assert_eq!(
            model.result.as_ref().unwrap()["matches"][0]["lineNumber"],
            1
        );
        assert!(model.result.as_ref().unwrap().get("query").is_none());
        let event = registry.event_projection(&result);
        assert_eq!(event.result.as_ref().unwrap()["query"], "target_symbol");
        assert!(event.result.as_ref().unwrap().get("omissions").is_none());
    }

    #[test]
    fn search_code_reports_skipped_files_without_changing_consumer_projections() {
        let fixture = TestWorkspace::new();
        fixture.write_file("src/lib.rs", "pub fn unrelated() {}\n");
        fixture.write_bytes(
            "large.txt",
            &vec![b'x'; usize::try_from(MAX_SEARCH_FILE_BYTES).unwrap() + 1],
        );
        fixture.write_bytes("not-utf8.txt", &[0xff, 0xfe]);
        let context = fixture.context();
        let registry = ToolRegistry::defaults_with_search(None);
        let call = AgentToolCall {
            id: "call-omissions".to_string(),
            tool: "search_code".to_string(),
            args: json!({ "query": "missing" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let raw = registry.execute(&context, &call);

        assert!(raw.ok, "{:?}", raw.error);
        let raw_value = raw.result.as_ref().unwrap();
        assert_eq!(raw_value["omissions"]["files"]["total"], 2);
        assert_eq!(
            raw_value["omissions"]["files"]["byReason"]["fileTooLarge"]["count"],
            1
        );
        assert_eq!(
            raw_value["omissions"]["files"]["byReason"]["fileTooLarge"]["maxBytes"],
            MAX_SEARCH_FILE_BYTES
        );
        assert_eq!(
            raw_value["omissions"]["files"]["byReason"]["readFailed"]["count"],
            1
        );
        assert_eq!(raw_value["truncated"], false);
        assert_eq!(raw_value["truncatedAtSource"], true);
        assert!(crate::tools::tool_result_truncated_at_source(&raw));

        let model = registry.model_projection(&raw);
        let model_value = model.result.as_ref().unwrap();
        assert!(model_value.get("query").is_none());
        assert_eq!(model_value["matchCount"], 0);
        assert_eq!(model_value["omissions"], raw_value["omissions"]);

        let event = registry.event_projection(&raw);
        assert_eq!(event.result.as_ref().unwrap()["query"], "missing");
        assert_eq!(
            event.result.as_ref().unwrap()["omissions"],
            raw_value["omissions"]
        );

        let trace = registry.trace_projection(&raw);
        assert_eq!(
            trace.result.as_ref().unwrap()["omissions"],
            raw_value["omissions"]
        );
    }

    #[test]
    fn search_code_reports_walk_cap_without_claiming_an_unknown_omitted_count() {
        let walk = WalkResult {
            entries: vec![WalkEntry {
                path: PathBuf::from("/workspace/src"),
                is_dir: true,
                size_bytes: 0,
            }],
            truncated: true,
        };

        let omissions = search_omissions(&walk, 0, 0).unwrap();

        assert_eq!(
            omissions["walk"]["reason"],
            "workspace_walk_entry_limit_reached"
        );
        assert_eq!(omissions["walk"]["returnedEntries"], 1);
        assert_eq!(omissions["walk"]["limitEntries"], MAX_WALK_ENTRIES);
        assert_eq!(omissions["walk"]["omittedCountKnown"], false);

        let raw = crate::protocol::AgentToolResult {
            call_id: "call-walk-cap".to_string(),
            tool: "search_code".to_string(),
            ok: true,
            result: Some(json!({
                "matches": [],
                "truncated": true,
                "omissions": omissions
            })),
            error: None,
        };
        assert!(crate::tools::tool_result_truncated_at_source(&raw));
    }

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = TEST_WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("my-copilot-agent-test-search-code-{unique}"));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write_file(&self, path: &str, content: &str) {
            self.write_bytes(path, content.as_bytes());
        }

        fn write_bytes(&self, path: &str, content: &[u8]) {
            let file_path = self.root.join(path);
            fs::create_dir_all(file_path.parent().unwrap()).unwrap();
            fs::write(file_path, content).unwrap();
        }

        fn context(&self) -> ToolExecutionContext {
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                conversation_id: None,
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("test".to_string()),
                    root_path: Some(self.root.to_string_lossy().to_string()),
                }),
                attachment_library: None,
                permissions: Default::default(),
            }))
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
