use super::{
    sanitize_limit,
    search_cursor::{
        decode_search_cursor, encode_search_cursor, largest_fitting_page_len,
        verify_search_snapshot, SearchCursorKind, SearchFingerprint,
    },
    walk_workspace_with_cancellation, AgentTool, ToolExecutionContext, WalkEntry, WalkResult,
    MAX_SEARCH_FILE_BYTES, MAX_SEARCH_LIMIT, MAX_WALK_ENTRIES,
};
use crate::protocol::{
    AgentError, AgentResult, AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;

const MAX_MODEL_MATCH_LINE_CHARS: usize = 2_000;

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
            description: "Search UTF-8 text content in an accessible file or directory. Selected read-only folders use @folders/<id>[/relative]. With no workspace, provide an absolute path or a system alias such as @desktop.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Text to search for." },
                    "path": { "type": "string", "description": "Optional workspace-relative or absolute directory/file, @folders/<id>[/relative], or @home/@desktop/@documents/@downloads. Defaults to workspace root when one exists." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_SEARCH_LIMIT },
                    "caseSensitive": { "type": "boolean" },
                    "cursor": { "type": "string", "description": "Opaque nextCursor from the previous page. Repeat the same query, path, limit, and caseSensitive values." }
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
        let request_hash = search_code_request_hash(
            context,
            &search_root,
            args.path.as_deref().unwrap_or("."),
            query,
            limit,
            case_sensitive,
        );
        let cursor = args
            .cursor
            .as_deref()
            .map(|cursor| decode_search_cursor(cursor, SearchCursorKind::Code, &request_hash))
            .transpose()?;
        let page_start = cursor.as_ref().map_or(0, |cursor| cursor.next_index);
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
        context.validate_folder_path(args.path.as_deref().unwrap_or("."))?;

        let mut skipped_too_large = 0usize;
        let mut read_failures = 0usize;
        let mut total_matches = 0usize;
        let mut snapshot = SearchFingerprint::new("search-code-snapshot-v1");
        snapshot.bool("walkTruncated", walk.truncated);
        snapshot.usize("walkEntries", walk.entries.len());
        for entry in walk.entries.iter().filter(|entry| !entry.is_dir) {
            cancellation_token.check()?;
            snapshot.string("path", &entry.path.to_string_lossy());
            snapshot.u64("sizeBytes", entry.size_bytes);
            if entry.size_bytes > MAX_SEARCH_FILE_BYTES {
                skipped_too_large = skipped_too_large.saturating_add(1);
                snapshot.string("readStatus", "fileTooLarge");
                continue;
            }

            let content = match fs::read_to_string(&entry.path) {
                Ok(content) => {
                    snapshot.string("readStatus", "ok");
                    snapshot.field("content", content.as_bytes());
                    content
                }
                Err(_) => {
                    read_failures = read_failures.saturating_add(1);
                    snapshot.string("readStatus", "readFailed");
                    continue;
                }
            };
            let display_path =
                context.display_path(args.path.as_deref().unwrap_or("."), &entry.path)?;

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

                let match_index = total_matches;
                total_matches = total_matches.saturating_add(1);
                snapshot.string("matchPath", &display_path);
                snapshot.usize("lineNumber", line_index.saturating_add(1));
                snapshot.string("line", line.trim());
                if match_index >= page_start && matches.len() < limit {
                    matches.push(json!({
                        "path": display_path.clone(),
                        "lineNumber": line_index + 1,
                        "line": line.trim()
                    }));
                }
            }
        }

        let snapshot_hash = snapshot.finish();
        verify_search_snapshot(cursor.as_ref(), &snapshot_hash, total_matches)?;
        let projected_matches = matches
            .iter()
            .map(project_search_code_match)
            .collect::<Vec<_>>();
        let omissions = search_omissions(&walk, skipped_too_large, read_failures);
        let truncated_at_source = omissions.is_some();
        let page_len = largest_fitting_page_len(context, matches.len(), |page_len| {
            search_code_model_page(
                total_matches,
                page_start,
                &projected_matches[..page_len],
                &request_hash,
                &snapshot_hash,
                walk.truncated,
                truncated_at_source,
                omissions.as_ref(),
            )
        })?;
        matches.truncate(page_len);
        let next_index = page_start.saturating_add(matches.len());
        let next_cursor = (next_index < total_matches)
            .then(|| {
                encode_search_cursor(
                    SearchCursorKind::Code,
                    &request_hash,
                    &snapshot_hash,
                    next_index,
                )
            })
            .transpose()?;
        let returned = matches.len();
        let omitted = total_matches.saturating_sub(returned);
        let mut output = json!({
            "query": query,
            "total": total_matches,
            "returned": returned,
            "omitted": omitted,
            "matches": matches,
            "truncated": next_cursor.is_some() || walk.truncated
        });
        if let Some(next_cursor) = next_cursor {
            output["nextCursor"] = Value::String(next_cursor);
        }
        if let Some(omissions) = omissions {
            output["omissions"] = omissions;
        }
        if truncated_at_source {
            output["truncatedAtSource"] = Value::Bool(true);
        }

        Ok(output)
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        let projected = result
            .result
            .as_ref()
            .map(search_code_model_projection_value);
        super::model_projection::compact_model_result(result, projected)
    }
}

fn search_code_model_projection_value(value: &Value) -> Value {
    let matches = value
        .get("matches")
        .and_then(Value::as_array)
        .map(|matches| {
            matches
                .iter()
                .map(project_search_code_match)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut output = serde_json::Map::new();
    output.insert(
        "matchCount".to_string(),
        Value::from(u64::try_from(matches.len()).unwrap_or(u64::MAX)),
    );
    if !matches.is_empty() {
        output.insert("matches".to_string(), Value::Array(matches));
    }
    for field in [
        "total",
        "returned",
        "omitted",
        "nextCursor",
        "truncated",
        "truncatedAtSource",
        "omissions",
    ] {
        super::model_projection::insert_field(&mut output, value, field);
    }
    Value::Object(output)
}

fn project_search_code_match(value: &Value) -> Value {
    let Some(source) = value.as_object() else {
        return value.clone();
    };
    let Some(line) = source.get("line").and_then(Value::as_str) else {
        return value.clone();
    };
    let total_chars = line.chars().count();
    if total_chars <= MAX_MODEL_MATCH_LINE_CHARS {
        return value.clone();
    }

    let retained = line
        .chars()
        .take(MAX_MODEL_MATCH_LINE_CHARS)
        .collect::<String>();
    let mut projected = source.clone();
    projected.insert(
        "line".to_string(),
        Value::String(format!(
            "{retained}…{} chars omitted",
            total_chars.saturating_sub(MAX_MODEL_MATCH_LINE_CHARS)
        )),
    );
    projected.insert("lineTruncated".to_string(), Value::Bool(true));
    projected.insert(
        "lineTotalChars".to_string(),
        Value::from(u64::try_from(total_chars).unwrap_or(u64::MAX)),
    );
    if let (Some(path), Some(line_number)) = (
        source.get("path").and_then(Value::as_str),
        source.get("lineNumber").and_then(Value::as_u64),
    ) {
        projected.insert(
            "continueWith".to_string(),
            json!({
                "tool": "read_file",
                "args": {
                    "path": path,
                    "startLine": line_number,
                    "maxLines": 1
                }
            }),
        );
    }
    Value::Object(projected)
}

#[allow(clippy::too_many_arguments)]
fn search_code_model_page(
    total: usize,
    page_start: usize,
    matches: &[Value],
    request_hash: &str,
    snapshot_hash: &str,
    walk_truncated: bool,
    truncated_at_source: bool,
    omissions: Option<&Value>,
) -> AgentResult<Value> {
    let next_index = page_start.saturating_add(matches.len());
    let next_cursor = (next_index < total)
        .then(|| {
            encode_search_cursor(
                SearchCursorKind::Code,
                request_hash,
                snapshot_hash,
                next_index,
            )
        })
        .transpose()?;
    let mut output = json!({
        "matchCount": matches.len(),
        "total": total,
        "returned": matches.len(),
        "omitted": total.saturating_sub(matches.len()),
        "matches": matches,
        "truncated": next_cursor.is_some() || truncated_at_source || walk_truncated
    });
    if let Some(next_cursor) = next_cursor {
        output["nextCursor"] = Value::String(next_cursor);
    }
    if truncated_at_source {
        output["truncatedAtSource"] = Value::Bool(true);
    }
    if let Some(omissions) = omissions {
        output["omissions"] = omissions.clone();
    }
    Ok(output)
}

fn search_code_request_hash(
    context: &ToolExecutionContext,
    search_root: &std::path::Path,
    input_path: &str,
    query: &str,
    limit: usize,
    case_sensitive: bool,
) -> String {
    let mut fingerprint = SearchFingerprint::new("search-code-request-v1");
    fingerprint.string("root", &search_root.to_string_lossy());
    fingerprint.string("projectId", context.project_id().unwrap_or(""));
    fingerprint.string("inputPath", input_path.trim());
    fingerprint.string("query", query);
    fingerprint.usize("limit", limit);
    fingerprint.bool("caseSensitive", case_sensitive);
    fingerprint.finish()
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
    cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::super::{
        ToolExecutionContext, ToolRegistry, WalkEntry, WalkResult, MAX_SEARCH_FILE_BYTES,
        MAX_WALK_ENTRIES,
    };
    use super::search_omissions;
    use crate::context::ContextCapacityDetector;
    use crate::protocol::{
        AgentApiStyle, AgentApprovalStatus, AgentRunContext, AgentToolCall, AgentToolResult,
        AgentWorkspaceContext,
    };
    use serde_json::{json, Value};
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
            exact_archive_file: None,
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

    #[test]
    fn search_code_cursor_pages_are_stable_without_duplicates_or_gaps() {
        let fixture = TestWorkspace::new();
        fixture.write_file(
            "src/lib.rs",
            "needle one\nneedle two\nneedle three\nneedle four\nneedle five\n",
        );
        let context = fixture.context();
        let registry = ToolRegistry::defaults_with_search(None);
        let mut cursor = None;
        let mut lines = Vec::new();
        let mut pages = 0usize;

        loop {
            let mut args = json!({ "query": "needle", "limit": 2 });
            if let Some(cursor) = cursor {
                args["cursor"] = json!(cursor);
            }
            let result = execute_search(&registry, &context, &format!("page-{pages}"), args);
            assert!(result.ok, "{:?}", result.error);
            let value = result.result.as_ref().unwrap();
            assert_eq!(value["total"], 5);
            assert_eq!(
                value["returned"],
                value["matches"].as_array().unwrap().len()
            );
            assert_eq!(
                value["omitted"].as_u64().unwrap(),
                5 - value["returned"].as_u64().unwrap()
            );
            lines.extend(
                value["matches"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| item["lineNumber"].as_u64().unwrap()),
            );
            pages = pages.saturating_add(1);
            cursor = value
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if cursor.is_none() {
                break;
            }
        }

        assert_eq!(pages, 3);
        assert_eq!(lines, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn search_code_cursor_rejects_changed_filters_and_stale_workspace_snapshot() {
        let fixture = TestWorkspace::new();
        fixture.write_file("src/lib.rs", "needle one\nneedle two\nneedle three\n");
        let context = fixture.context();
        let registry = ToolRegistry::defaults_with_search(None);
        let first = execute_search(
            &registry,
            &context,
            "cursor-first",
            json!({ "query": "needle", "limit": 1 }),
        );
        let cursor = first.result.as_ref().unwrap()["nextCursor"]
            .as_str()
            .unwrap()
            .to_string();

        let mismatch = execute_search(
            &registry,
            &context,
            "cursor-mismatch",
            json!({ "query": "other", "limit": 1, "cursor": cursor }),
        );
        assert!(!mismatch.ok);
        assert!(mismatch
            .error
            .as_deref()
            .is_some_and(|error| error.contains("不匹配")));

        let cursor = first.result.as_ref().unwrap()["nextCursor"]
            .as_str()
            .unwrap()
            .to_string();
        fixture.write_file(
            "src/lib.rs",
            "needle one\nneedle two changed\nneedle three\n",
        );
        let stale = execute_search(
            &registry,
            &context,
            "cursor-stale",
            json!({ "query": "needle", "limit": 1, "cursor": cursor }),
        );
        assert!(!stale.ok);
        assert!(stale
            .error
            .as_deref()
            .is_some_and(|error| error.contains("发生变化")));
    }

    #[test]
    fn search_code_long_results_fit_before_the_central_gate_and_cursor_tracks_visible_items() {
        let fixture = TestWorkspace::new();
        let content = (0..40)
            .map(|index| format!("needle-{index}-{}", "x".repeat(8_000)))
            .collect::<Vec<_>>()
            .join("\n");
        fixture.write_file("src/long.rs", &content);
        let detector =
            ContextCapacityDetector::for_model("test-model", AgentApiStyle::OpenAiCompatible, &[]);
        let context = fixture.context().with_text_output_budget(
            detector.text_budget(crate::context::MODEL_TOOL_RESULT_MAX_TOKENS),
        );
        let gate = detector.model_tool_result_gate();
        let registry = ToolRegistry::defaults_with_search(None);
        let mut cursor = None;
        let mut lines = Vec::new();
        let mut page = 0usize;

        loop {
            let mut args = json!({ "query": "needle", "limit": 200 });
            if let Some(cursor) = cursor {
                args["cursor"] = json!(cursor);
            }
            let result = execute_search(&registry, &context, &format!("long-{page}"), args);
            assert!(result.ok, "{:?}", result.error);
            let model = registry.model_projection(&result);
            let gated = gate.project(&result.call_id, false, &model, None);
            assert!(
                !gated.truncated,
                "search page should already fit before the central gate"
            );
            let model_value = model.result.as_ref().unwrap();
            assert!(model_value["matches"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| {
                    item["lineTruncated"] == true
                        && item.get("continueWith").is_some()
                        && item.get("lineTotalChars").is_some()
                }));
            lines.extend(
                result.result.as_ref().unwrap()["matches"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| item["lineNumber"].as_u64().unwrap()),
            );
            cursor = result.result.as_ref().unwrap()["nextCursor"]
                .as_str()
                .map(ToString::to_string);
            page = page.saturating_add(1);
            if cursor.is_none() {
                break;
            }
        }

        assert!(page > 1);
        assert_eq!(lines, (1..=40).collect::<Vec<_>>());
    }

    fn execute_search(
        registry: &ToolRegistry,
        context: &ToolExecutionContext,
        call_id: &str,
        args: Value,
    ) -> AgentToolResult {
        registry.execute(
            context,
            &AgentToolCall {
                id: call_id.to_string(),
                tool: "search_code".to_string(),
                args,
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        )
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
