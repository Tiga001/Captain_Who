//! Minimal-mode wording over the existing Tool contracts.
//!
//! The full definitions remain owned by their Tools. This projection changes only descriptions:
//! execution, approval metadata, parameters and validation limits are shared with the full mode.
//! Keep the edits explicit so a new Tool or schema field retains its original guidance by default.

use crate::protocol::AgentToolDefinition;
use serde_json::Value;

/// Apply before freezing a Run's stable ToolSet. Skill activation must not replace these schemas
/// later in the Run; optional managed Skill command inputs are therefore present from the start.
pub(crate) fn apply_minimal_tool_descriptions(definitions: &mut [AgentToolDefinition]) {
    for definition in definitions {
        let Some(description) = minimal_description(&definition.name) else {
            continue;
        };
        definition.description = description.to_string();
        for &(pointer, text) in schema_descriptions(&definition.name) {
            replace_description(&mut definition.input_schema, pointer, text);
        }
        if definition.name == "apply_patch" {
            minimize_file_change_branches(&mut definition.input_schema);
        }
    }
}

fn minimal_description(name: &str) -> Option<&'static str> {
    Some(match name {
        "read_file" => concat!(
            "Read an authorized regular UTF-8 file and issue this Run's fileChangeTarget. ",
            "Without a range, read the whole file within the output budget; truncated pages provide lossless continuation."
        ),
        "apply_patch" => concat!(
            "Change one UTF-8 file. The only root argument is request: select its matching branch, content or structured edits as specified; no raw unified diff. ",
            "Direct apply creates/updates/deletes. create omits observationId (including missing-file receipts) and needs no pre-read; Host atomically refuses overwrite and success issues fileChangeTarget. ",
            "update/delete/begin-update require this Run's exact fileChangeTarget.filePath and observationId from read_file or successful apply/commit. If absent, read the exact target; listings/search cannot substitute. ",
            "Successful update/delete or Staged update commit renews the same ID only after its Tool Result: reuse in the next model response; never share that ID across writes in one batch. ",
            "Failure/rejection/cancellation/conflict/outcome_unknown do not renew it. Reread for missing fileChangeTarget, observationRefreshRequired, unknown/externally changed contents or match/conflict errors; follow continueWith. ",
            "For long content/multi-step assembly use Staged: begin/create starts empty without observationId; begin/update uses valid credentials and modify/rewrite; Staged delete is unsupported. ",
            "Copy latest Host transactionId/nextIndex/draftRevision; never invent cursors or replay persisted chunks. append/edit changes only the draft, not file observations; only successful commit issues/renews fileChangeTarget. ",
            "Follow allowedNextActions: drafting/ready allow only the same transaction's append/edit/commit/status/abort; waiting_approval/applying/outcome_unknown allow only status, never edits or abort. ",
            "Settle with commit/abort before user-visible narration. After applied/already_applied, later edits need a new transaction using the successful commit's fileChangeTarget; other terminal states or missing credentials require read_file."
        ),
        "run_command" => concat!(
            "Run a bounded non-interactive shell command for queries, builds, tests or programs. Follow cwd before the first call. ",
            "Host policy executes, requests approval or denies; it is not an OS sandbox. Host owns the initial yield: do not add a timeout just to confirm startup. ",
            "running is not success; use command_session for required results."
        ),
        "command_session" => concat!(
            "Wait for or interrupt the original authorized run_command Session; no arbitrary stdin or duplicate command. ",
            "Wait for required results, not natural exit of GUI apps/servers. Waiting is one quiet phase: no repeated polling or before/after narration; Timeline shows output. Speak at terminal status or new actionable facts/decisions. ",
            "Latest status supersedes earlier run_command results: starting/running are non-terminal; exited/interrupted/timed_out/failed mean the process stopped. ",
            "outcome_unknown ends tracking but does not establish process outcome: stop polling, claim neither success nor continued execution, and never replay the command. Background output/exit never starts a model turn."
        ),
        "conversation_history" => "Read this conversation's durable history: {} lists recent completed turns, query searches, open follows a returned location.",
        "read_image" => "Read one authorized image as visual input. Pass only path: copy the exact user/tool location, never invent source, URI or attachment ID. Host checks authorization and integrity.",
        "workspace_map" => "Inspect an authorized directory: bounded tree, languages, important files and entrypoint/test/documentation candidates; no file contents. Selected read-only folders use @folders/<id>[/relative].",
        "search_files" => "Find paths by case-insensitive name/path substring. Read UTF-8 kind=file results with read_file; inspect kind=directory with workspace_map.focusPath, never read_file. Selected read-only folders use @folders/<id>[/relative].",
        "search_code" => "Search UTF-8 contents of an authorized file or directory, including selected read-only folders at @folders/<id>[/relative].",
        "skills_activate" => "Load a matching or explicitly requested Skill's full instructions and revision-bound resources from this Run's catalog. Activation grants no file/command/network/approval permission.",
        "attachments_list" => "List this chat's files/images; use returned @attachments readPath unchanged.",
        "attachments_list_project" => concat!(
            "List files/images from other authorized chats in this project or Agent tree, excluding this chat; ",
            "use returned @attachments readPath unchanged."
        ),
        _ => return None,
    })
}

/// JSON pointers address schema nodes, not model argument paths. Only an existing textual
/// description is replaced: missing fields, numeric constraints and external schemas are untouched.
fn replace_description(schema: &mut Value, pointer: &str, text: &str) {
    if let Some(description) = schema
        .pointer_mut(pointer)
        .and_then(|node| node.get_mut("description"))
        .filter(|description| description.is_string())
    {
        *description = Value::String(text.to_string());
    }
}

fn schema_descriptions(name: &str) -> &'static [(&'static str, &'static str)] {
    match name {
        "read_file" => &[
            ("/properties/path", "Authorized regular UTF-8 file: workspace-relative only with a workspace, selected read-only folders use @folders/<id>/relative/path, otherwise absolute or @home/@desktop/@documents/@downloads. Also exact @attachments readPath, browser-download: or published artifact://. Directories: workspace_map.focusPath; read permission/ownership apply."),
            ("/properties/startLine", "First line (1-based); default beginning."),
            ("/properties/startByte", "Copy nextStartByte; pair with expectedRevision, never startLine."),
            ("/properties/expectedRevision", "Only with startByte: copy the preceding page's revision. If changed, restart reading; never splice file versions."),
            ("/properties/maxLines", "Soft line bound, no fixed maximum; output token budget still applies."),
        ],
        "run_command" => &[
            ("/properties/command", "Non-interactive shell text; CRLF/CR become LF. Host checks each newline/pipeline/&&/||/; segment. Bounded non-writing heredocs need quoted delimiters, e.g. <<'PY'; unquoted/shell-interpreter heredocs, here-strings, background execution and NUL are denied."),
            ("/properties/cwd", "Before the first call, check World State workspace.binding. With workspace: omit for root or use a relative directory. Without workspace: require an existing absolute directory or @home/@desktop/@documents/@downloads[/child], even for absolute command paths; never relative or '.'. Absolute paths/aliases require write=all. Only a backend-recognized command whose currently activated Skill explicitly supplies a Host-owned private directory may omit cwd."),
            ("/properties/reason", "Purpose and expected result."),
            ("/properties/observe", "Best-effort per activated Skill; paths relative to cwd. Grants no permissions."),
            ("/properties/observe/properties/expectedOutputs", "Exact outputs only; no sibling scan. Observation does not change command success."),
            ("/properties/observe/properties/additionalRoots", "Extra files/directories; external recursive scans require read=all."),
            ("/properties/runtimeProfile", "Only if the currently activated Skill explicitly instructs; else omit. Host verifies/freezes runtime identity. Never guess profiles or supply package versions; no extra permissions or PATH fallback."),
            ("/properties/inputs", "Authorized read-only inputs at $MYCOPILOT_INPUT_ROOT/<mountPath>; each item has path and optional mountPath, never source. Host freezes hash/size before approval and revalidates before execution. Copy user/tool paths, never private Host storage paths; supports browser downloads."),
            ("/properties/inputs/items/properties/path", "Exact authorized workspace/absolute/system-alias path, @attachments, browser-download:, image-artifact://, artifact://, or revision-bound skill:// reference."),
            ("/properties/inputs/items/properties/mountPath", "Safe relative mount path; default source filename."),
        ],
        "command_session" => &[
            ("/properties/sessionId", "Exact sessionId from run_command."),
            ("/properties/action", "wait (default): Host-bounded quiet observation. interrupt: controlled interrupt."),
        ],
        "conversation_history" => &[
            ("/properties/query", "Historical phrase/topic/path/identifier/tool/error."),
            ("/properties/open", "Exact opaque hist_v1_ location from a previous result; never modify or invent."),
        ],
        "read_image" => &[
            ("/properties/path", "Exact workspace-relative/absolute/system-alias path, @folders/<id>/relative/path, @attachments, browser-download:, image-artifact:// or revision-bound skill:// reference."),
        ],
        "workspace_map" => &[
            ("/properties/focusPath", "Authorized directory: workspace-relative/absolute, @folders/<id>[/relative], or @home/@desktop/@documents/@downloads. Defaults to workspace root; without one, specify absolute path/alias."),
            ("/properties/maxDepth", "Tree depth from focusPath; default 4."),
            ("/properties/maxEntries", "Maximum tree entries; default 200."),
            ("/properties/includeFiles", "Include files (default true); statistics always count them."),
        ],
        "search_files" => &[
            ("/properties/query", "Case-insensitive name/path substring."),
            ("/properties/path", "Authorized directory: workspace-relative/absolute, @folders/<id>[/relative], or @home/@desktop/@documents/@downloads; default workspace root. Without workspace, specify absolute path/alias."),
            ("/properties/cursor", "Exact nextCursor; repeat query/path/limit unchanged."),
        ],
        "search_code" => &[
            ("/properties/query", "Text to find."),
            ("/properties/path", "Authorized file/directory: workspace-relative/absolute, @folders/<id>[/relative], or @home/@desktop/@documents/@downloads; default workspace root. Without workspace, specify absolute path/alias."),
            ("/properties/cursor", "Exact nextCursor; repeat query/path/limit/caseSensitive unchanged."),
        ],
        "skills_activate" => &[
            ("/properties/skillRef", "Copy the exact ref from this Run's backend_available_skills into skillRef; never invent or reuse across Runs."),
            ("/properties/reason", "Brief user-facing activation reason."),
        ],
        "attachments_list" | "attachments_list_project" => &[
            ("/properties/kind", "Optional file/image filter."),
            ("/properties/limit", "Maximum returned attachments."),
            ("/properties/cursor", "Exact nextCursor; repeat kind/limit unchanged. If expired, omit cursor and list again."),
        ],
        _ => &[],
    }
}

fn minimize_file_change_branches(schema: &mut Value) {
    replace_description(
        schema,
        "/properties/request",
        "One matching branch; no extra fields.",
    );
    let Some(branches) = schema
        .pointer_mut("/properties/request/oneOf")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for branch in branches.iter_mut() {
        let is_create = branch
            .pointer("/properties/operation/enum/0")
            .and_then(Value::as_str)
            == Some("create");
        let action = branch
            .pointer("/properties/action/enum/0")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        replace_description(
            branch,
            "/properties/filePath",
            if is_create {
                "New authorized target path."
            } else {
                "Exact fileChangeTarget.filePath."
            },
        );
        for &(pointer, text) in &[
            (
                "/properties/observationId",
                "Exact fileChangeTarget.observationId.",
            ),
            ("/properties/transactionId", "Exact Host transactionId."),
            ("/properties/index", "Latest Host nextIndex."),
            (
                "/properties/expectedDraftRevision",
                "Latest Host draftRevision.",
            ),
            ("/properties/summary", "Short change summary."),
            (
                "/properties/strategy",
                "modify starts from observed content; rewrite from an empty draft.",
            ),
        ] {
            replace_description(branch, pointer, text);
        }
        let content_description = match action.as_str() {
            "apply" if is_create => "Complete UTF-8 content, at most 32 KiB; empty allowed.",
            "apply" => {
                "Complete replacement, at most 32 KiB UTF-8; larger: begin/update strategy=rewrite."
            }
            "append" => "Non-empty UTF-8 chunk: at most 1 MiB; transaction total at most 4 MiB.",
            _ => continue,
        };
        replace_description(branch, "/properties/content", content_description);
    }
    // Direct and Staged both retain the same exact-edit variants and their distinct size limits.
    for branch in branches.iter_mut() {
        let staged = branch
            .pointer("/properties/action/enum/0")
            .and_then(Value::as_str)
            == Some("edit");
        replace_description(
            branch,
            "/properties/edits",
            if staged {
                "1-128 ordered exact-byte edits; no trimming/normalization/fuzzy matching. replace needs one match unless replaceAll=true. Result at most 4 MiB."
            } else {
                "1-128 ordered exact-byte edits; no trimming/normalization/fuzzy matching. replace needs one match unless replaceAll=true. Result at most 240,000 UTF-8 bytes."
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextTextBudget;
    use crate::tools::ToolRegistry;

    fn full_definitions() -> Vec<AgentToolDefinition> {
        let mut registry = ToolRegistry::defaults_with_search(None);
        registry.register_conversation_history();
        registry.definitions()
    }

    fn without_descriptions(value: &mut Value) {
        match value {
            Value::Object(object) => {
                if object.get("description").is_some_and(Value::is_string) {
                    object.remove("description");
                }
                for child in object.values_mut() {
                    without_descriptions(child);
                }
            }
            Value::Array(array) => array.iter_mut().for_each(without_descriptions),
            _ => {}
        }
    }

    #[test]
    fn minimal_wording_preserves_every_wire_constraint_and_full_definition() {
        let full = full_definitions();
        let original = serde_json::to_value(&full).unwrap();
        let mut minimal = full.clone();
        apply_minimal_tool_descriptions(&mut minimal);
        let mut full_contract = original.clone();
        let mut minimal_contract = serde_json::to_value(&minimal).unwrap();
        without_descriptions(&mut full_contract);
        without_descriptions(&mut minimal_contract);
        assert_eq!(minimal_contract, full_contract);
        assert_eq!(serde_json::to_value(full_definitions()).unwrap(), original);

        let once = serde_json::to_value(&minimal).unwrap();
        apply_minimal_tool_descriptions(&mut minimal);
        assert_eq!(serde_json::to_value(&minimal).unwrap(), once);
        let patch = minimal
            .iter()
            .find(|tool| tool.name == "apply_patch")
            .unwrap();
        assert_eq!(
            patch.input_schema["properties"]["request"]["oneOf"]
                .as_array()
                .unwrap()
                .len(),
            11
        );
    }

    #[test]
    fn external_and_unlisted_tool_wording_is_untouched() {
        let mut definitions = full_definitions();
        let mut external = definitions
            .iter()
            .find(|tool| tool.name == "read_file")
            .unwrap()
            .clone();
        external.name = "mcp.vendor.read_file".to_string();
        definitions.push(external);
        let original = definitions
            .iter()
            .filter(|tool| minimal_description(&tool.name).is_none())
            .cloned()
            .collect::<Vec<_>>();
        apply_minimal_tool_descriptions(&mut definitions);
        let unchanged = definitions
            .into_iter()
            .filter(|tool| minimal_description(&tool.name).is_none())
            .collect::<Vec<_>>();
        assert_eq!(
            serde_json::to_value(unchanged).unwrap(),
            serde_json::to_value(original).unwrap()
        );
    }

    #[test]
    fn unknown_schema_guidance_and_missing_descriptions_are_untouched() {
        let mut definitions = full_definitions();
        let file = definitions
            .iter_mut()
            .find(|tool| tool.name == "read_file")
            .unwrap();
        file.input_schema["properties"]["futureField"] = serde_json::json!({
            "type": "string",
            "description": "An unknown field must retain its original guidance."
        });
        file.input_schema["properties"]["path"]
            .as_object_mut()
            .unwrap()
            .remove("description");
        let original_schema = file.input_schema.clone();
        apply_minimal_tool_descriptions(&mut definitions);
        let schema = &definitions
            .iter()
            .find(|tool| tool.name == "read_file")
            .unwrap()
            .input_schema;
        assert_eq!(
            schema["properties"]["futureField"],
            original_schema["properties"]["futureField"]
        );
        assert_eq!(
            schema["properties"]["path"],
            original_schema["properties"]["path"]
        );
    }

    #[test]
    fn minimal_guidance_keeps_file_receipts_sessions_and_skill_input_contracts() {
        let mut definitions = full_definitions();
        apply_minimal_tool_descriptions(&mut definitions);
        let tool = |name: &str| definitions.iter().find(|tool| tool.name == name).unwrap();
        let patch = tool("apply_patch");
        for rule in [
            "only root argument is request",
            "content or structured edits as specified; no raw unified diff",
            "create omits observationId",
            "including missing-file receipts",
            "no pre-read",
            "atomically refuses overwrite",
            "success issues fileChangeTarget",
            "this Run's exact fileChangeTarget.filePath and observationId",
            "read the exact target; listings/search cannot substitute",
            "renews the same ID only after its Tool Result",
            "reuse in the next model response",
            "never share that ID across writes in one batch",
            "Failure/rejection/cancellation/conflict/outcome_unknown do not renew",
            "observationRefreshRequired",
            "externally changed contents",
            "follow continueWith",
            "begin/create starts empty without observationId",
            "begin/update uses valid credentials and modify/rewrite",
            "Staged delete is unsupported",
            "Copy latest Host transactionId/nextIndex/draftRevision",
            "never invent cursors or replay persisted chunks",
            "append/edit changes only the draft",
            "only successful commit issues/renews fileChangeTarget",
            "allowedNextActions",
            "drafting/ready allow only the same transaction's append/edit/commit/status/abort",
            "waiting_approval/applying/outcome_unknown allow only status",
            "never edits or abort",
            "commit/abort before user-visible narration",
            "applied/already_applied",
            "new transaction using the successful commit's fileChangeTarget",
            "other terminal states or missing credentials require read_file",
        ] {
            assert!(
                patch.description.contains(rule),
                "missing FileChange guidance: {rule}"
            );
        }
        let command = serde_json::to_string(tool("run_command")).unwrap();
        for rule in [
            "without",
            "World State workspace.binding",
            "Before the first call",
            "existing absolute directory",
            "even for absolute command paths",
            "never relative or '.'",
            "write=all",
            "Only a backend-recognized command whose currently activated Skill explicitly supplies a Host-owned private directory may omit cwd",
            "quoted delimiters",
            "unquoted/shell-interpreter heredocs, here-strings, background execution and NUL are denied",
            "Only if the currently activated Skill explicitly instructs",
            "else omit",
            "Host verifies/freezes runtime identity",
            "Never guess profiles or supply package versions; no extra permissions or PATH fallback",
            "run_command",
            "$MYCOPILOT_INPUT_ROOT/",
            "freezes hash/size",
            "revalidates before execution",
            "never private Host storage paths",
            "browser-download:",
            "skill://",
            "expectedOutputs",
            "additionalRoots",
        ] {
            // The capitalized sentence opener must not alter the permission rule being tested.
            assert!(
                command.to_lowercase().contains(&rule.to_lowercase()),
                "missing command guidance: {rule}"
            );
        }
        for rule in [
            "original authorized run_command Session",
            "no arbitrary stdin or duplicate command",
            "not natural exit of GUI apps/servers",
            "Waiting is one quiet phase",
            "no repeated polling or before/after narration",
            "Speak at terminal status or new actionable facts/decisions",
            "Latest status supersedes",
            "starting/running are non-terminal",
            "exited/interrupted/timed_out/failed mean the process stopped",
            "outcome_unknown ends tracking but does not establish process outcome",
            "stop polling, claim neither success nor continued execution",
            "never replay the command",
            "Background output/exit never starts a model turn",
        ] {
            assert!(
                tool("command_session").description.contains(rule),
                "missing Session guidance: {rule}"
            );
        }
        // History trust belongs to the system prompt; specialized format workflows belong
        // to activated Skills. These tools retain only scope and exact returned readPath; the system routes readers.
        assert!(tool("conversation_history")
            .description
            .contains("this conversation's durable history"));
        for name in ["attachments_list", "attachments_list_project"] {
            let description = &tool(name).description;
            assert!(description.contains("returned @attachments readPath unchanged"));
        }
        assert!(tool("attachments_list").description.contains("this chat's"));
        assert!(tool("attachments_list_project")
            .description
            .contains("other authorized chats in this project or Agent tree, excluding this chat"));
        assert!(schema_descriptions("skills_activate").iter().any(
            |(pointer, description)| *pointer == "/properties/skillRef"
                && description
                    .contains("exact ref from this Run's backend_available_skills into skillRef")
                && description.contains("never invent or reuse across Runs")
        ));
        assert!(
            tool("read_file").input_schema["properties"]["expectedRevision"]["description"]
                .as_str()
                .unwrap()
                .contains("never splice file versions")
        );
    }

    #[test]
    fn ordinary_tool_descriptions_leave_format_workflows_to_activated_skills() {
        fn collect_descriptions(value: &Value, output: &mut Vec<String>) {
            match value {
                Value::Object(object) => {
                    if let Some(description) = object.get("description").and_then(Value::as_str) {
                        output.push(description.to_string());
                    }
                    for child in object.values() {
                        collect_descriptions(child, output);
                    }
                }
                Value::Array(array) => {
                    for child in array {
                        collect_descriptions(child, output);
                    }
                }
                _ => {}
            }
        }

        let full = full_definitions();
        let mut minimal = full.clone();
        apply_minimal_tool_descriptions(&mut minimal);
        for definitions in [full, minimal] {
            for definition in definitions.iter().filter(|tool| {
                matches!(
                    tool.name.as_str(),
                    "run_command" | "attachments_list" | "attachments_list_project"
                )
            }) {
                let mut descriptions = Vec::new();
                collect_descriptions(
                    &serde_json::to_value(definition).unwrap(),
                    &mut descriptions,
                );
                for description in descriptions {
                    for specialized in [
                        "Office",
                        "PDF",
                        "Builder",
                        "Word",
                        "Excel",
                        "PowerPoint",
                        ".docx",
                        ".xlsx",
                        ".pptx",
                        "--output",
                    ] {
                        assert!(
                            !description.contains(specialized),
                            "{} leaked Skill-only guidance: {specialized}",
                            definition.name
                        );
                    }
                }
            }
            // These selectors are wire contracts, not capability advertisements. Moving
            // instructions into Skills must never remove or reinterpret their values.
            let command = definitions
                .iter()
                .find(|tool| tool.name == "run_command")
                .unwrap();
            assert_eq!(
                command.input_schema["properties"]["runtimeProfile"]["enum"],
                serde_json::json!(["documents", "spreadsheets", "presentations"])
            );
            assert_eq!(
                command.input_schema["properties"]["observe"]["properties"]["kinds"]["items"]
                    ["enum"],
                serde_json::json!(["office"])
            );
        }
    }

    #[test]
    fn minimal_wording_reduces_default_tool_schema_budget() {
        let full = full_definitions()
            .into_iter()
            .filter(|tool| minimal_description(&tool.name).is_some())
            .collect::<Vec<_>>();
        let mut minimal = full.clone();
        apply_minimal_tool_descriptions(&mut minimal);
        let budget = ContextTextBudget::heuristic(u64::MAX);
        let full_tokens = budget.estimate_tool_definitions(&full);
        let minimal_tokens = budget.estimate_tool_definitions(&minimal);
        assert!(
            minimal_tokens * 100 <= full_tokens * 90,
            "minimal={minimal_tokens}, full={full_tokens}"
        );
    }
}
