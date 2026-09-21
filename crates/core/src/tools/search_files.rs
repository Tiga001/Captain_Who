use super::{
    sanitize_limit,
    search_cursor::{
        decode_search_cursor, encode_search_cursor, largest_fitting_page_len,
        verify_search_snapshot, SearchCursorKind, SearchFingerprint,
    },
    walk_workspace_with_cancellation, AgentTool, ToolExecutionContext, MAX_SEARCH_LIMIT,
    MAX_WALK_ENTRIES,
};
use crate::protocol::{AgentError, AgentResult, AgentToolDefinition, AgentToolSafety};
use serde::Deserialize;
use serde_json::{json, Value};

pub(super) struct SearchFilesTool;

impl AgentTool for SearchFilesTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "search_files".to_string(),
            description: "Find files or directories by path or file name. Route every match by its kind: kind=file may be passed to read_file when it is UTF-8 text; kind=directory must be inspected with workspace_map.focusPath, never read_file. Selected folders add read access; use their absolute paths."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Case-insensitive path or file-name substring." },
                    "path": { "type": "string", "description": "Optional workspace-relative or absolute directory, or @home/@desktop/@documents/@downloads. Defaults to workspace root when one exists." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_SEARCH_LIMIT },
                    "cursor": { "type": "string", "description": "Opaque nextCursor from the previous page. Repeat the same query, path, and limit values." }
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
        let args: SearchFilesArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("search_files 参数无效：{error}")))?;
        let query = args.query.trim();
        if query.is_empty() {
            return Err(AgentError::new("search_files.query 不能为空。"));
        }

        let limit = sanitize_limit(args.limit);
        let root = match args.path.as_deref().filter(|path| !path.trim().is_empty()) {
            Some(path) => context.resolve_existing_path(path)?,
            None => context.workspace_root_optional()?.ok_or_else(|| {
                AgentError::new(
                    "当前没有 workspace；search_files.path 必须指定绝对目录或系统路径别名。",
                )
            })?,
        };
        if !root.is_dir() {
            return Err(AgentError::new("search_files.path 必须是目录。"));
        }
        let request_hash = search_files_request_hash(
            context,
            &root,
            args.path.as_deref().unwrap_or("."),
            query,
            limit,
        );
        let cursor = args
            .cursor
            .as_deref()
            .map(|cursor| decode_search_cursor(cursor, SearchCursorKind::Files, &request_hash))
            .transpose()?;
        let page_start = cursor.as_ref().map_or(0, |cursor| cursor.next_index);
        let needle = query.to_ascii_lowercase();
        let mut matches = Vec::new();
        let cancellation_token = context.cancellation_token();
        let walk = walk_workspace_with_cancellation(&root, &cancellation_token)?;
        context.validate_folder_path(args.path.as_deref().unwrap_or("."))?;
        let mut total_matches = 0usize;
        let mut snapshot = SearchFingerprint::new("search-files-snapshot-v1");
        snapshot.bool("walkTruncated", walk.truncated);
        snapshot.usize("walkEntries", walk.entries.len());

        for entry in &walk.entries {
            cancellation_token.check()?;
            let display_path =
                context.display_path(args.path.as_deref().unwrap_or("."), &entry.path)?;
            snapshot.string("path", &display_path);
            snapshot.bool("isDirectory", entry.is_dir);
            snapshot.u64("sizeBytes", entry.size_bytes);
            if !display_path.to_ascii_lowercase().contains(&needle) {
                continue;
            }

            let match_index = total_matches;
            total_matches = total_matches.saturating_add(1);
            if match_index >= page_start && matches.len() < limit {
                matches.push(json!({
                    "path": display_path,
                    "kind": if entry.is_dir { "directory" } else { "file" },
                    "sizeBytes": entry.size_bytes
                }));
            }
        }

        let snapshot_hash = snapshot.finish();
        verify_search_snapshot(cursor.as_ref(), &snapshot_hash, total_matches)?;
        let page_len = largest_fitting_page_len(context, matches.len(), |page_len| {
            search_files_page(
                query,
                total_matches,
                page_start,
                &matches[..page_len],
                &request_hash,
                &snapshot_hash,
                walk.truncated,
                &walk,
            )
        })?;
        matches.truncate(page_len);
        let next_index = page_start.saturating_add(matches.len());
        let next_cursor = (next_index < total_matches)
            .then(|| {
                encode_search_cursor(
                    SearchCursorKind::Files,
                    &request_hash,
                    &snapshot_hash,
                    next_index,
                )
            })
            .transpose()?;
        let mut output = json!({
            "query": query,
            "total": total_matches,
            "returned": matches.len(),
            "omitted": total_matches.saturating_sub(matches.len()),
            "matches": matches,
            "truncated": next_cursor.is_some() || walk.truncated
        });
        if let Some(next_cursor) = next_cursor {
            output["nextCursor"] = Value::String(next_cursor);
        }
        if walk.truncated {
            output["truncatedAtSource"] = Value::Bool(true);
            output["omissions"] = json!({
                "walk": {
                    "reason": "workspace_walk_entry_limit_reached",
                    "returnedEntries": walk.entries.len(),
                    "limitEntries": MAX_WALK_ENTRIES,
                    "omittedCountKnown": false
                }
            });
        }
        Ok(output)
    }
}

#[allow(clippy::too_many_arguments)]
fn search_files_page(
    query: &str,
    total: usize,
    page_start: usize,
    matches: &[Value],
    request_hash: &str,
    snapshot_hash: &str,
    walk_truncated: bool,
    walk: &super::WalkResult,
) -> AgentResult<Value> {
    let next_index = page_start.saturating_add(matches.len());
    let next_cursor = (next_index < total)
        .then(|| {
            encode_search_cursor(
                SearchCursorKind::Files,
                request_hash,
                snapshot_hash,
                next_index,
            )
        })
        .transpose()?;
    let mut output = json!({
        "query": query,
        "total": total,
        "returned": matches.len(),
        "omitted": total.saturating_sub(matches.len()),
        "matches": matches,
        "truncated": next_cursor.is_some() || walk_truncated
    });
    if let Some(next_cursor) = next_cursor {
        output["nextCursor"] = Value::String(next_cursor);
    }
    if walk_truncated {
        output["truncatedAtSource"] = Value::Bool(true);
        output["omissions"] = json!({
            "walk": {
                "reason": "workspace_walk_entry_limit_reached",
                "returnedEntries": walk.entries.len(),
                "limitEntries": MAX_WALK_ENTRIES,
                "omittedCountKnown": false
            }
        });
    }
    Ok(output)
}

fn search_files_request_hash(
    context: &ToolExecutionContext,
    root: &std::path::Path,
    input_path: &str,
    query: &str,
    limit: usize,
) -> String {
    let mut fingerprint = SearchFingerprint::new("search-files-request-v1");
    fingerprint.string("root", &root.to_string_lossy());
    fingerprint.string("projectId", context.project_id().unwrap_or(""));
    fingerprint.string("inputPath", input_path.trim());
    fingerprint.string("query", query);
    fingerprint.usize("limit", limit);
    fingerprint.finish()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchFilesArgs {
    query: String,
    path: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::super::{ToolExecutionContext, ToolRegistry};
    use crate::context::ContextCapacityDetector;
    use crate::protocol::{
        AgentApiStyle, AgentApprovalStatus, AgentRunContext, AgentToolCall, AgentWorkspaceContext,
    };
    use serde_json::{json, Value};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn search_files_finds_paths_by_name() {
        let fixture = TestWorkspace::new();
        fixture.write_file("src/main.rs", "fn main() {}\n");
        fixture.write_file("README.md", "hello\n");
        let context = fixture.context();
        let registry = ToolRegistry::defaults_with_search(None);
        let definition = registry.definition_for("search_files").unwrap();
        assert!(definition.description.contains("kind=file"));
        assert!(definition.description.contains("read_file"));
        assert!(definition.description.contains("kind=directory"));
        assert!(definition.description.contains("workspace_map.focusPath"));
        let call = AgentToolCall {
            id: "call-1".to_string(),
            tool: "search_files".to_string(),
            args: json!({ "query": "main", "limit": 10 }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let result = registry.execute(&context, &call);

        assert!(result.ok, "{:?}", result.error);
        let result = result.result.unwrap();
        assert_eq!(result["matches"][0]["path"], "src/main.rs");
        assert_eq!(result["matches"][0]["kind"], "file");
    }

    #[test]
    fn search_files_cursor_pages_without_duplicates_or_gaps() {
        let fixture = TestWorkspace::new();
        for index in 0..5 {
            fixture.write_file(&format!("src/match-{index}.rs"), "fn main() {}\n");
        }
        let context = fixture.context();
        let registry = ToolRegistry::defaults_with_search(None);
        let mut cursor = None;
        let mut paths = Vec::new();

        loop {
            let mut args = json!({ "query": "match-", "limit": 2 });
            if let Some(cursor) = cursor {
                args["cursor"] = json!(cursor);
            }
            let result = registry.execute(
                &context,
                &AgentToolCall {
                    id: format!("page-{}", paths.len()),
                    tool: "search_files".to_string(),
                    args,
                    approval_status: AgentApprovalStatus::NotRequired,
                    reason: None,
                },
            );
            assert!(result.ok, "{:?}", result.error);
            let value = result.result.as_ref().unwrap();
            assert_eq!(value["total"], 5);
            assert_eq!(
                value["omitted"].as_u64().unwrap(),
                5 - value["returned"].as_u64().unwrap()
            );
            paths.extend(
                value["matches"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| item["path"].as_str().unwrap().to_string()),
            );
            cursor = value
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if cursor.is_none() {
                break;
            }
        }

        assert_eq!(
            paths,
            (0..5)
                .map(|index| format!("src/match-{index}.rs"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn search_files_cursor_is_bound_to_workspace_identity() {
        let first_fixture = TestWorkspace::new();
        first_fixture.write_file("match-a.rs", "a");
        first_fixture.write_file("match-b.rs", "b");
        let second_fixture = TestWorkspace::new();
        second_fixture.write_file("match-a.rs", "a");
        second_fixture.write_file("match-b.rs", "b");
        let registry = ToolRegistry::defaults_with_search(None);
        let first = registry.execute(
            &first_fixture.context(),
            &AgentToolCall {
                id: "workspace-first".to_string(),
                tool: "search_files".to_string(),
                args: json!({ "query": "match-", "limit": 1 }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );
        let cursor = first.result.as_ref().unwrap()["nextCursor"]
            .as_str()
            .unwrap();
        let mismatch = registry.execute(
            &second_fixture.context(),
            &AgentToolCall {
                id: "workspace-second".to_string(),
                tool: "search_files".to_string(),
                args: json!({ "query": "match-", "limit": 1, "cursor": cursor }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );

        assert!(!mismatch.ok);
        assert!(mismatch
            .error
            .as_deref()
            .is_some_and(|error| error.contains("不匹配")));
    }

    #[test]
    fn search_files_many_long_results_fit_before_the_central_gate() {
        let fixture = TestWorkspace::new();
        let directory = format!("{}-directory", "nested".repeat(20));
        for index in 0..120 {
            fixture.write_file(
                &format!(
                    "{directory}/match-{index:03}-{}.rs",
                    "long-file-name".repeat(10)
                ),
                "x",
            );
        }
        let detector =
            ContextCapacityDetector::for_model("test-model", AgentApiStyle::OpenAiCompatible, &[]);
        let context = fixture.context().with_text_output_budget(
            detector.text_budget(crate::context::MODEL_TOOL_RESULT_MAX_TOKENS),
        );
        let gate = detector.model_tool_result_gate();
        let registry = ToolRegistry::defaults_with_search(None);
        let mut cursor = None;
        let mut paths = Vec::new();
        let mut pages = 0usize;

        loop {
            let mut args = json!({ "query": "match-", "limit": 200 });
            if let Some(cursor) = cursor {
                args["cursor"] = json!(cursor);
            }
            let result = registry.execute(
                &context,
                &AgentToolCall {
                    id: format!("long-files-{pages}"),
                    tool: "search_files".to_string(),
                    args,
                    approval_status: AgentApprovalStatus::NotRequired,
                    reason: None,
                },
            );
            assert!(result.ok, "{:?}", result.error);
            let model = registry.model_projection(&result);
            let gated = gate.project(&result.call_id, false, &model, None);
            assert!(
                !gated.truncated,
                "search_files page should fit before the central gate"
            );
            paths.extend(
                result.result.as_ref().unwrap()["matches"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| item["path"].as_str().unwrap().to_string()),
            );
            cursor = result.result.as_ref().unwrap()["nextCursor"]
                .as_str()
                .map(ToString::to_string);
            pages = pages.saturating_add(1);
            if cursor.is_none() {
                break;
            }
        }

        assert!(pages > 1);
        assert_eq!(paths.len(), 120);
        let mut unique = paths.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 120);
    }

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = TEST_WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("my-copilot-agent-test-search-files-{unique}"));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write_file(&self, path: &str, content: &str) {
            let file_path = self.root.join(path);
            fs::create_dir_all(file_path.parent().unwrap()).unwrap();
            fs::write(file_path, content).unwrap();
        }

        fn context(&self) -> ToolExecutionContext {
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: None,
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    folders: Vec::new(),
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
