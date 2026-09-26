use std::path::{Path, PathBuf};

const PRODUCTION_ROOTS: &[&str] = &[
    "crates/core/src",
    "crates/core-server/src",
    "crates/protocol-rs/src",
    "packages/protocol/src",
    "packages/host-api/src",
    "src/main",
    "src/preload",
    "src/renderer/src",
    "src/shared",
    "packages/protocol/fixtures",
];

const RETIRED_FILE_WRITER_PATHS: &[&str] = &[
    "crates/core/src/patch.rs",
    "crates/core/src/file_write.rs",
    "crates/core/src/tools/write_file.rs",
    "crates/core/src/tools/write_file_stream.rs",
    "crates/core/src/tools/apply_patch_diff.rs",
    "crates/core/src/tools/apply_patch_paths.rs",
    "crates/core-server/src/application/agent/action_execution/file_authorization.rs",
    "crates/core-server/src/application/agent/tests/file_write_permissions.rs",
];

const RETIRED_FILE_WRITER_MARKERS: &[&str] = &[
    "WriteFileTool",
    "WriteFilePhase",
    "AgentFileWrite",
    "AgentFileDraft",
    "AgentProposedAction::Diff",
    "AgentProposedAction::FileWrite",
    "readFileDraft",
    "getFileWriteDiff",
    "file_write_preview",
    "file_draft_updated",
    "patchResult",
    "fileWriteResult",
    "agent_file_drafts",
    "agent_file_draft_chunks",
    "agent_file_draft_operations",
    "git apply",
    "apply_unified_patch",
    "parse_unified_patch",
    "raw_patch",
    "args.get(\"patch\")",
];

struct CurrentSemanticAllowance {
    file: &'static str,
    line_marker: &'static str,
    expected_occurrences: usize,
    reason: &'static str,
}

const UNSUPPORTED_TOOL_DIAGNOSTIC_ALLOWANCE: CurrentSemanticAllowance =
    CurrentSemanticAllowance {
        file: "crates/core/src/skills/tool_reference_lint.rs",
        line_marker:
            "const UNSUPPORTED_MODEL_TOOL_REFERENCES: &[&str] = &[\"write_file\"]",
        expected_occurrences: 1,
        reason: "Untrusted Skills diagnose the exact retired identifier without registering or rewriting it.",
    };

const FLAT_WIRE_REJECTION_ALLOWANCE: CurrentSemanticAllowance = CurrentSemanticAllowance {
    file: "crates/core/src/tools/apply_patch/tests.rs",
    line_marker: "args: json!({\"action\":\"apply\",\"content\":\"PRIVATE_FLAT_CANARY\"})",
    expected_occurrences: 1,
    reason: "One test-only malformed call proves the retired flat Wire is rejected and redacted before any public projection.",
};

/// These are current risk classifications, not a model-visible Tool, Wire compatibility alias, or
/// FileChange persistence shape. Every allowance names one file and one exact line marker so a new
/// occurrence cannot hide behind a directory-level exception.
const CURRENT_FILE_WRITE_RISK_ALLOWLIST: &[CurrentSemanticAllowance] = &[
    CurrentSemanticAllowance {
        file: "crates/core/src/office/types/operations.rs",
        line_marker: "OfficeOperationAccess::FileWrite",
        expected_occurrences: 1,
        reason: "Office mutations and render outputs retain their current write-risk classification.",
    },
    CurrentSemanticAllowance {
        file: "crates/core/src/office/types/operations.rs",
        line_marker: "    FileWrite,",
        expected_occurrences: 1,
        reason: "OfficeOperationAccess names the current Office write-risk variant.",
    },
    CurrentSemanticAllowance {
        file: "crates/core/src/office/execution.rs",
        line_marker: "OfficeOperationAccess::FileWrite",
        expected_occurrences: 3,
        reason: "The Office executor routes current write-risk operations through managed output authorization.",
    },
    CurrentSemanticAllowance {
        file: "crates/core/src/tools/office.rs",
        line_marker: "OfficeOperationAccess::FileWrite",
        expected_occurrences: 4,
        reason: "The Office Tool prepares current write-risk operations for FileChange authorization.",
    },
    CurrentSemanticAllowance {
        file: "crates/core-server/src/application/agent/action_execution/runners/dispatch.rs",
        line_marker: "OfficeOperationAccess::FileWrite",
        expected_occurrences: 2,
        reason: "The Core Server revalidates the current Office write-risk classification before execution.",
    },
    CurrentSemanticAllowance {
        file: "crates/core/src/protocol/skills.rs",
        line_marker: "FileWrite,",
        expected_occurrences: 1,
        reason: "AgentBuiltinMcpToolRiskKind retains the current Builtin MCP file-write risk variant.",
    },
    CurrentSemanticAllowance {
        file: "crates/protocol-rs/src/managed_playwright_bridge.rs",
        line_marker: "FileWrite,",
        expected_occurrences: 1,
        reason: "Managed Playwright bridge DTO retains the current file-write risk variant.",
    },
    CurrentSemanticAllowance {
        file: "crates/core-server/src/application/mcp/builtin_capability_runtime.rs",
        line_marker: "Core::FileWrite => BuiltinMcpToolRiskKindDto::FileWrite",
        expected_occurrences: 1,
        reason: "Builtin MCP maps the current Core risk enum to its strict DTO.",
    },
    CurrentSemanticAllowance {
        file: "crates/core-server/src/application/mcp/playwright_manifest.rs",
        line_marker: "Risk::FileWrite",
        expected_occurrences: 1,
        reason: "The signed Managed Playwright manifest declares current file-write risk.",
    },
    CurrentSemanticAllowance {
        file: "packages/protocol/src/agent/approvals.ts",
        line_marker: "'file_write'",
        expected_occurrences: 1,
        reason: "The strict Agent Builtin MCP risk union retains its current wire value.",
    },
    CurrentSemanticAllowance {
        file: "packages/protocol/src/agentParsers/builtinApprovals.ts",
        line_marker: "'file_write'",
        expected_occurrences: 1,
        reason: "The Agent parser validates the current Builtin MCP risk wire value.",
    },
    CurrentSemanticAllowance {
        file: "packages/protocol/src/mcp/managedPlaywrightBridge.ts",
        line_marker: "'file_write'",
        expected_occurrences: 2,
        reason: "The Managed Playwright bridge validates its current risk wire value.",
    },
    CurrentSemanticAllowance {
        file: "src/main/mcp/managedPlaywrightSensitivePolicy.ts",
        line_marker: "'file_write'",
        expected_occurrences: 1,
        reason: "Main classifies current Managed Playwright file-write operations as sensitive.",
    },
    CurrentSemanticAllowance {
        file: "src/renderer/src/features/chat/components/BuiltinMcpToolApprovalCard.tsx",
        line_marker: "file_write:",
        expected_occurrences: 1,
        reason: "Renderer maps the current Builtin MCP risk value to safe localized approval copy.",
    },
    CurrentSemanticAllowance {
        file: "src/shared/i18n/frontendTranslations.enUS.ts",
        line_marker: "'agent.builtinMcpApproval.risk.file_write':",
        expected_occurrences: 1,
        reason: "English UI copy names the current Builtin MCP file-write risk classification.",
    },
    CurrentSemanticAllowance {
        file: "src/shared/i18n/frontendTranslations.frFR.ts",
        line_marker: "'agent.builtinMcpApproval.risk.file_write':",
        expected_occurrences: 1,
        reason: "French UI copy names the current Builtin MCP file-write risk classification.",
    },
    CurrentSemanticAllowance {
        file: "src/shared/i18n/frontendTranslations.itIT.ts",
        line_marker: "'agent.builtinMcpApproval.risk.file_write':",
        expected_occurrences: 1,
        reason: "Italian UI copy names the current Builtin MCP file-write risk classification.",
    },
    CurrentSemanticAllowance {
        file: "src/shared/i18n/frontendTranslations.jaJP.ts",
        line_marker: "'agent.builtinMcpApproval.risk.file_write':",
        expected_occurrences: 1,
        reason: "Japanese UI copy names the current Builtin MCP file-write risk classification.",
    },
    CurrentSemanticAllowance {
        file: "src/shared/i18n/frontendTranslations.koKR.ts",
        line_marker: "'agent.builtinMcpApproval.risk.file_write':",
        expected_occurrences: 1,
        reason: "Korean UI copy names the current Builtin MCP file-write risk classification.",
    },
    CurrentSemanticAllowance {
        file: "src/shared/i18n/frontendTranslations.ruRU.ts",
        line_marker: "'agent.builtinMcpApproval.risk.file_write':",
        expected_occurrences: 1,
        reason: "Russian UI copy names the current Builtin MCP file-write risk classification.",
    },
    CurrentSemanticAllowance {
        file: "src/shared/i18n/frontendTranslations.zhCN.ts",
        line_marker: "'agent.builtinMcpApproval.risk.file_write':",
        expected_occurrences: 1,
        reason: "Simplified Chinese UI copy names the current Builtin MCP file-write risk classification.",
    },
    CurrentSemanticAllowance {
        file: "src/shared/i18n/frontendTranslations.zhTW.ts",
        line_marker: "'agent.builtinMcpApproval.risk.file_write':",
        expected_occurrences: 1,
        reason: "Traditional Chinese UI copy names the current Builtin MCP file-write risk classification.",
    },
    CurrentSemanticAllowance {
        file: "packages/protocol/src/agent/office.ts",
        line_marker: "| 'fileWrite'",
        expected_occurrences: 1,
        reason: "The current strict Office access DTO retains its lower-camel write-risk value.",
    },
    CurrentSemanticAllowance {
        file: "packages/protocol/src/agentParsers/actionPayloads.ts",
        line_marker: "['readOnly', 'fileWrite']",
        expected_occurrences: 1,
        reason: "The strict Agent parser validates the current Office access risk value.",
    },
    CurrentSemanticAllowance {
        file: "src/renderer/src/features/storage/persistedAgentRunOfficeValidators.ts",
        line_marker: "value.access === 'fileWrite'",
        expected_occurrences: 1,
        reason: "The current Renderer Office projection validates the strict Office access risk value.",
    },
    CurrentSemanticAllowance {
        file: "crates/core/src/storage/chat_repository/agent_run_projection/validation/skills_office.rs",
        line_marker: "Some(\"readOnly\" | \"fileWrite\")",
        expected_occurrences: 1,
        reason: "The current durable chat projection validates the strict Office access risk value before retaining it.",
    },
];

const CURRENT_GENERATED_DIFF_ALLOWLIST: &[CurrentSemanticAllowance] = &[
    CurrentSemanticAllowance {
        file: "crates/core/src/file_change/planner.rs",
        line_marker: ".unified_diff()",
        expected_occurrences: 1,
        reason: "FileChange planner generates the frozen approval view from exact Base and Target content.",
    },
    CurrentSemanticAllowance {
        file: "crates/core/src/file_change_support.rs",
        line_marker: ".unified_diff()",
        expected_occurrences: 1,
        reason: "FileChange support regenerates a paginated review view from the canonical transaction snapshot.",
    },
    CurrentSemanticAllowance {
        file: "crates/core/src/git_review/diff.rs",
        line_marker: ".unified_diff()",
        expected_occurrences: 1,
        reason: "Git review renders a read-only user review Diff and never executes it as FileChange input.",
    },
    CurrentSemanticAllowance {
        file: "crates/core/src/git_review/turn.rs",
        line_marker: ".unified_diff()",
        expected_occurrences: 1,
        reason: "Git review turn projection renders a read-only Diff and never executes it as FileChange input.",
    },
];

#[test]
fn retired_model_writer_wire_events_and_storage_cannot_reenter_production() {
    let root = workspace_root();
    let mut violations = Vec::new();
    let mut diagnostic_occurrences = 0;
    for (relative, production) in production_sources(&root) {
        for (line_index, line) in production.lines().enumerate() {
            if line.contains("write_file") {
                if relative == UNSUPPORTED_TOOL_DIAGNOSTIC_ALLOWANCE.file
                    && line.contains(UNSUPPORTED_TOOL_DIAGNOSTIC_ALLOWANCE.line_marker)
                {
                    diagnostic_occurrences += 1;
                } else {
                    violations.push(format!(
                        "{relative}:{} contains retired provider-visible tool name `write_file`",
                        line_index + 1
                    ));
                }
            }
            for marker in RETIRED_FILE_WRITER_MARKERS {
                if line.contains(marker) {
                    violations.push(format!(
                        "{relative}:{} contains retired FileChange marker `{marker}`",
                        line_index + 1
                    ));
                }
            }
        }
    }
    assert!(!UNSUPPORTED_TOOL_DIAGNOSTIC_ALLOWANCE.reason.is_empty());
    if diagnostic_occurrences != UNSUPPORTED_TOOL_DIAGNOSTIC_ALLOWANCE.expected_occurrences {
        violations.push(format!(
            "{}:{} expected {} occurrence(s), observed {} ({})",
            UNSUPPORTED_TOOL_DIAGNOSTIC_ALLOWANCE.file,
            UNSUPPORTED_TOOL_DIAGNOSTIC_ALLOWANCE.line_marker,
            UNSUPPORTED_TOOL_DIAGNOSTIC_ALLOWANCE.expected_occurrences,
            diagnostic_occurrences,
            UNSUPPORTED_TOOL_DIAGNOSTIC_ALLOWANCE.reason
        ));
    }

    assert!(
        violations.is_empty(),
        "retired file-writer interfaces escaped into production:\n{}\n\
         Do not add an alias, compatibility parser, old storage reader, or dual UI path.",
        violations.join("\n")
    );
}

#[test]
fn current_apply_patch_calls_cannot_reintroduce_flat_wire() {
    let root = workspace_root();
    let mut rust_files = Vec::new();
    for relative_root in ["crates/core/src", "crates/core-server/src"] {
        collect_files_with_extensions(&root.join(relative_root), &["rs"], &mut rust_files);
    }
    let flat_call_prefix = [
        "tool:",
        "\"apply_patch\"",
        ".to_string(),args:json!({",
        "\"action\"",
    ]
    .concat();
    let flat_action_read = ["call.args[", "\"action\"", "]"].concat();
    let flat_action_get = ["call.args.get(", "\"action\"", ")"].concat();
    let flat_provider_arguments = ["\\\"arguments\\\":\\\"{", "\\\\\\\"action"].concat();
    let mut violations = Vec::new();
    let mut allowed = 0_usize;
    for path in rust_files {
        let relative = normalized_relative(&root, &path);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"));
        let compact = source
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect::<String>();
        let flat_calls = compact.matches(&flat_call_prefix).count();
        if flat_calls > 0 {
            if relative == FLAT_WIRE_REJECTION_ALLOWANCE.file
                && source
                    .matches(FLAT_WIRE_REJECTION_ALLOWANCE.line_marker)
                    .count()
                    == FLAT_WIRE_REJECTION_ALLOWANCE.expected_occurrences
                && flat_calls == FLAT_WIRE_REJECTION_ALLOWANCE.expected_occurrences
            {
                allowed += flat_calls;
            } else {
                violations.push(format!(
                    "{relative} contains {flat_calls} positive or unallowlisted flat apply_patch call(s)"
                ));
            }
        }
        for forbidden in [
            &flat_action_read,
            &flat_action_get,
            &flat_provider_arguments,
        ] {
            if compact.contains(forbidden) {
                violations.push(format!(
                    "{relative} reads or emits the retired flat apply_patch Wire marker `{forbidden}`"
                ));
            }
        }
    }
    assert!(!FLAT_WIRE_REJECTION_ALLOWANCE.reason.trim().is_empty());
    assert_eq!(
        allowed, FLAT_WIRE_REJECTION_ALLOWANCE.expected_occurrences,
        "the exact malformed-flat rejection fixture must remain singular"
    );
    assert!(
        violations.is_empty(),
        "flat apply_patch Wire escaped its exact rejection-only allowlist:\n{}",
        violations.join("\n")
    );
}

#[test]
fn current_file_write_risk_names_are_confined_to_the_exact_allowlist() {
    let root = workspace_root();
    let mut violations = Vec::new();
    let mut observed = vec![0_usize; CURRENT_FILE_WRITE_RISK_ALLOWLIST.len()];
    for (relative, production) in production_sources(&root) {
        for (line_index, line) in production.lines().enumerate() {
            if !line.contains("FileWrite")
                && !line.contains("file_write")
                && !line.contains("fileWrite")
            {
                continue;
            }
            let matches = CURRENT_FILE_WRITE_RISK_ALLOWLIST
                .iter()
                .enumerate()
                .filter(|(_, allowance)| {
                    relative == allowance.file && line.contains(allowance.line_marker)
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                violations.push(format!(
                    "{relative}:{} matches {} allowlist entries for current/retired file-write name: {}",
                    line_index + 1,
                    matches.len(),
                    line.trim()
                ));
            } else {
                observed[matches[0]] += 1;
            }
        }
    }

    for (index, allowance) in CURRENT_FILE_WRITE_RISK_ALLOWLIST.iter().enumerate() {
        assert!(
            !allowance.reason.trim().is_empty(),
            "allowance {}:{} must document its current semantic reason",
            allowance.file,
            allowance.line_marker
        );
        if observed[index] != allowance.expected_occurrences {
            violations.push(format!(
                "allowance {}:{} expected {} occurrence(s), observed {} ({})",
                allowance.file,
                allowance.line_marker,
                allowance.expected_occurrences,
                observed[index],
                allowance.reason
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "file_write/FileWrite source-boundary violations:\n{}\n\
         Add a single exact file+marker+reason only for a current risk classification.",
        violations.join("\n")
    );
}

#[test]
fn model_guidance_and_bundled_authoring_packages_name_only_apply_patch() {
    let root = workspace_root();
    let prompt_path = root.join("crates/core/src/prompts.rs");
    let prompt = rust_without_cfg_test_modules(
        &std::fs::read_to_string(&prompt_path).expect("prompt source must be readable"),
    );
    assert!(!prompt.contains("write_file"));
    assert!(prompt.contains("Direct 使用 request.action=apply"));
    assert!(prompt.contains("同一 apply_patch Staged 模式"));
    assert!(prompt.contains("create 不得提供 observationId，也不必先 read_file"));
    assert!(prompt.contains("把输入的同一个 observationId 续约到写后状态"));
    assert!(prompt.contains("必须先对准确目标路径使用 read_file"));

    let mut guidance_files = Vec::new();
    collect_files_with_extensions(
        &root.join("crates/core/src/skills/bundled"),
        &["md", "json", "py", "mjs"],
        &mut guidance_files,
    );
    collect_files_with_extensions(&root.join("docs"), &["md"], &mut guidance_files);
    for path in guidance_files {
        let relative = normalized_relative(&root, &path);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"));
        assert!(
            !source.contains("write_file"),
            "{relative} still instructs readers or models to use the retired writer"
        );
    }

    for (relative, builder_template, editor_template) in [
        (
            "crates/core/src/skills/bundled/documents/office-capability.json",
            "templates/builder.py",
            "templates/editor.py",
        ),
        (
            "crates/core/src/skills/bundled/spreadsheets/office-capability.json",
            "templates/builder.py",
            "templates/editor.py",
        ),
        (
            "crates/core/src/skills/bundled/presentations/office-capability.json",
            "templates/builder.mjs",
            "templates/editor.mjs",
        ),
    ] {
        let value: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join(relative))
                .unwrap_or_else(|error| panic!("failed to read {relative}: {error}")),
        )
        .unwrap_or_else(|error| panic!("invalid {relative}: {error}"));
        assert_eq!(
            value["modes"]["script"]["editTools"],
            serde_json::json!(["apply_patch"]),
            "{relative} must expose exactly one ordinary text editor"
        );
        assert_eq!(
            value["modes"]["script"]["builderTemplate"],
            builder_template
        );
        assert_eq!(value["modes"]["script"]["editorTemplate"], editor_template);
        assert_eq!(value["modes"]["script"]["executionTool"], "run_command");
        assert_eq!(value["modes"]["script"]["routes"]["create"], "builder");
        assert_eq!(value["modes"]["script"]["routes"]["editExisting"], "editor");
    }
}

#[test]
fn raw_unified_patch_is_a_view_not_a_model_execution_authority() {
    let root = workspace_root();
    for relative in RETIRED_FILE_WRITER_PATHS {
        assert!(
            !root.join(relative).exists(),
            "retired writer source path `{relative}` must not be recreated"
        );
    }
    let apply_patch_schema = rust_without_cfg_test_modules(
        &std::fs::read_to_string(root.join("crates/core/src/tools/apply_patch/schema.rs")).unwrap(),
    );
    assert_section_excludes(
        &apply_patch_schema,
        "fn patch_input_schema() -> Value {",
        "fn structured_edits_schema() -> Value {",
        "\"patch\"",
    );
    let apply_patch_request = rust_without_cfg_test_modules(
        &std::fs::read_to_string(root.join("crates/core/src/tools/apply_patch/request.rs"))
            .unwrap(),
    );
    assert_section_excludes(
        &apply_patch_request,
        "enum ApplyPatchArgs {",
        "fn parse_args(",
        "patch:",
    );

    let planner = std::fs::read_to_string(root.join("crates/core/src/file_change/planner.rs"))
        .expect("FileChange planner must be readable");
    assert!(
        planner.contains(".unified_diff()"),
        "FileChange must retain generated diff as the approval view"
    );
    let mut observed = vec![0_usize; CURRENT_GENERATED_DIFF_ALLOWLIST.len()];
    let mut violations = Vec::new();
    for (relative, production) in production_sources(&root) {
        for (line_index, line) in production.lines().enumerate() {
            if !line.contains(".unified_diff()") {
                continue;
            }
            let matches = CURRENT_GENERATED_DIFF_ALLOWLIST
                .iter()
                .enumerate()
                .filter(|(_, allowance)| {
                    relative == allowance.file && line.contains(allowance.line_marker)
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if matches.len() == 1 {
                observed[matches[0]] += 1;
            } else {
                violations.push(format!(
                    "{relative}:{} generated Diff call is outside the exact view-only allowlist",
                    line_index + 1
                ));
            }
        }
    }
    for (index, allowance) in CURRENT_GENERATED_DIFF_ALLOWLIST.iter().enumerate() {
        assert!(!allowance.reason.trim().is_empty());
        if observed[index] != allowance.expected_occurrences {
            violations.push(format!(
                "allowance {}:{} expected {} occurrence(s), observed {} ({})",
                allowance.file,
                allowance.line_marker,
                allowance.expected_occurrences,
                observed[index],
                allowance.reason
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "generated Diff authority escaped its exact view-only allowlist:\n{}",
        violations.join("\n")
    );
    for relative in [
        "crates/core/src/file_change/committer.rs",
        "crates/core/src/file_change/policy.rs",
        "crates/core/src/tools/apply_patch.rs",
        "crates/core/src/tools/file_change_staged.rs",
        "crates/core-server/src/application/agent/action_execution/file_change_authorization.rs",
    ] {
        let production = production_file(&root, relative);
        for forbidden in [
            "git apply",
            "apply_unified_patch",
            "parse_unified_patch",
            "raw_patch",
            "args.get(\"patch\")",
        ] {
            assert!(
                !production.contains(forbidden),
                "{relative} contains retired raw-patch execution authority `{forbidden}`"
            );
        }
    }
}

#[test]
fn file_change_execution_does_not_classify_state_from_error_text() {
    let root = workspace_root();
    for relative in [
        "crates/core/src/file_change/error.rs",
        "crates/core/src/file_change/committer.rs",
        "crates/core/src/file_change/policy.rs",
        "crates/core/src/tools/apply_patch.rs",
        "crates/core/src/tools/file_change_staged.rs",
        "crates/core-server/src/application/agent/action_execution.rs",
        "crates/core-server/src/application/agent/action_execution/file_change_authorization.rs",
        "crates/core-server/src/application/agent/pending_action_store.rs",
    ] {
        let production = production_file(&root, relative);
        assert!(
            !production.contains("error.contains("),
            "{relative} classifies FileChange state from error text instead of typed error/outcome"
        );
    }
}

fn assert_section_excludes(source: &str, start: &str, end: &str, forbidden: &str) {
    let start_index = source
        .find(start)
        .unwrap_or_else(|| panic!("missing section start `{start}`"));
    let remainder = &source[start_index..];
    let end_index = remainder
        .find(end)
        .unwrap_or_else(|| panic!("missing section end `{end}`"));
    assert!(
        !remainder[..end_index].contains(forbidden),
        "section `{start}` contains forbidden marker `{forbidden}`"
    );
}

fn production_sources(workspace_root: &Path) -> Vec<(String, String)> {
    let mut sources = Vec::new();
    for relative_root in PRODUCTION_ROOTS {
        collect_files_with_extensions(
            &workspace_root.join(relative_root),
            &["rs", "ts", "tsx", "sql", "json", "md", "py", "mjs", "js"],
            &mut sources,
        );
    }
    sources
        .into_iter()
        .filter_map(|path| {
            let relative = normalized_relative(workspace_root, &path);
            if is_test_source(&relative) {
                return None;
            }
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"));
            let production = if relative.ends_with(".rs") {
                rust_without_cfg_test_modules(&source)
            } else {
                source
            };
            Some((relative, production))
        })
        .collect()
}

fn production_file(workspace_root: &Path, relative: &str) -> String {
    let source = std::fs::read_to_string(workspace_root.join(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"));
    if relative.ends_with(".rs") {
        rust_without_cfg_test_modules(&source)
    } else {
        source
    }
}

fn collect_files_with_extensions(root: &Path, extensions: &[&str], output: &mut Vec<PathBuf>) {
    let mut entries = std::fs::read_dir(root)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", root.display()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("failed to enumerate {}: {error}", root.display()));
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_files_with_extensions(&path, extensions, output);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extensions.contains(&extension))
        {
            output.push(path);
        }
    }
}

fn normalized_relative(workspace_root: &Path, path: &Path) -> String {
    path.strip_prefix(workspace_root)
        .expect("source path must remain inside workspace")
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_test_source(relative: &str) -> bool {
    relative.contains("/tests/")
        || relative.ends_with("/tests.rs")
        || relative.ends_with(".test.ts")
        || relative.ends_with(".test.tsx")
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("core-server crate must live under workspace/crates")
        .to_path_buf()
}

fn rust_without_cfg_test_modules(source: &str) -> String {
    const CFG_TEST: &str = "#[cfg(test)]";
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    while let Some(relative_start) = source[cursor..].find(CFG_TEST) {
        let start = cursor + relative_start;
        let after_attribute = start + CFG_TEST.len();
        let item_start = skip_ascii_whitespace(source, after_attribute);
        if !source[item_start..].starts_with("mod ") {
            output.push_str(&source[cursor..after_attribute]);
            cursor = after_attribute;
            continue;
        }
        let Some(relative_open) = source[item_start..].find('{') else {
            output.push_str(&source[cursor..after_attribute]);
            cursor = after_attribute;
            continue;
        };
        let open = item_start + relative_open;
        let Some(end) = matching_rust_brace(source, open) else {
            panic!("unterminated #[cfg(test)] module in architecture source scan");
        };
        output.push_str(&source[cursor..start]);
        output.extend(
            source[start..end]
                .chars()
                .filter(|character| *character == '\n'),
        );
        cursor = end;
    }
    output.push_str(&source[cursor..]);
    output
}

fn skip_ascii_whitespace(source: &str, mut cursor: usize) -> usize {
    let bytes = source.as_bytes();
    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor
}

fn matching_rust_brace(source: &str, open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut cursor = open;
    let mut depth = 0_u32;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'/' if bytes.get(cursor + 1) == Some(&b'/') => {
                cursor += 2;
                while cursor < bytes.len() && bytes[cursor] != b'\n' {
                    cursor += 1;
                }
            }
            b'/' if bytes.get(cursor + 1) == Some(&b'*') => {
                cursor = skip_block_comment(bytes, cursor + 2)?;
            }
            b'"' => cursor = skip_quoted(bytes, cursor + 1, b'"')?,
            b'\'' if is_char_literal(bytes, cursor) => {
                cursor = skip_quoted(bytes, cursor + 1, b'\'')?;
            }
            b'r' if raw_string_hash_count(bytes, cursor).is_some() => {
                let hashes = raw_string_hash_count(bytes, cursor)?;
                cursor = skip_raw_string(bytes, cursor, hashes)?;
            }
            b'{' => {
                depth = depth.checked_add(1)?;
                cursor += 1;
            }
            b'}' => {
                depth = depth.checked_sub(1)?;
                cursor += 1;
                if depth == 0 {
                    return Some(cursor);
                }
            }
            _ => cursor += 1,
        }
    }
    None
}

fn skip_block_comment(bytes: &[u8], mut cursor: usize) -> Option<usize> {
    let mut depth = 1_u32;
    while cursor < bytes.len() {
        if bytes.get(cursor..cursor + 2) == Some(b"/*") {
            depth = depth.checked_add(1)?;
            cursor += 2;
        } else if bytes.get(cursor..cursor + 2) == Some(b"*/") {
            depth = depth.checked_sub(1)?;
            cursor += 2;
            if depth == 0 {
                return Some(cursor);
            }
        } else {
            cursor += 1;
        }
    }
    None
}

fn skip_quoted(bytes: &[u8], mut cursor: usize, quote: u8) -> Option<usize> {
    while cursor < bytes.len() {
        if bytes[cursor] == b'\\' {
            cursor = cursor.checked_add(2)?;
        } else if bytes[cursor] == quote {
            return Some(cursor + 1);
        } else {
            cursor += 1;
        }
    }
    None
}

fn is_char_literal(bytes: &[u8], cursor: usize) -> bool {
    matches!(
        bytes.get(cursor + 1..),
        Some([b'\\', _, b'\'', ..]) | Some([_, b'\'', ..])
    )
}

fn raw_string_hash_count(bytes: &[u8], cursor: usize) -> Option<usize> {
    let mut probe = cursor.checked_add(1)?;
    while bytes.get(probe) == Some(&b'#') {
        probe += 1;
    }
    (bytes.get(probe) == Some(&b'"')).then_some(probe - cursor - 1)
}

fn skip_raw_string(bytes: &[u8], cursor: usize, hashes: usize) -> Option<usize> {
    let mut probe = cursor.checked_add(hashes + 2)?;
    while probe < bytes.len() {
        if bytes[probe] == b'"'
            && bytes
                .get(probe + 1..probe + 1 + hashes)
                .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
        {
            return Some(probe + 1 + hashes);
        }
        probe += 1;
    }
    None
}
