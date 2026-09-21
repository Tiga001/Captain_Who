use super::{relative_display, walk_workspace_with_cancellation, AgentTool, ToolExecutionContext};
use crate::cancellation::AgentCancellationToken;
use crate::protocol::{
    AgentError, AgentResult, AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use crate::resource_locator::ResourceLocator;
use serde::Deserialize;
use serde_json::{json, Value};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const DEFAULT_MAX_DEPTH: usize = 4;
const MAX_MAP_DEPTH: usize = 8;
const DEFAULT_MAX_ENTRIES: usize = 200;
const MAX_MAP_ENTRIES: usize = 1_000;
const MAX_LANGUAGE_STATS: usize = 20;
const MAX_IMPORTANT_FILES: usize = 80;
const MAX_CANDIDATES: usize = 40;
const MAX_TOP_DIRECTORIES: usize = 30;

pub(super) struct WorkspaceMapTool;

impl AgentTool for WorkspaceMapTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "workspace_map".to_string(),
            description: "Summarize an authorized directory structure: bounded file tree, language statistics, important files, entrypoint candidates, tests, and documentation candidates without reading file contents. Defaults to the workspace root when one exists; selected read-only folders use @folders/<id>[/relative].".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "focusPath": {
                        "type": "string",
                        "description": "Optional workspace-relative or absolute directory, @folders/<id>[/relative], or @home/@desktop/@documents/@downloads. Defaults to the workspace root when one exists. Availability depends on the current read permission."
                    },
                    "maxDepth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_MAP_DEPTH,
                        "description": "Maximum tree depth relative to focusPath. Defaults to 4."
                    },
                    "maxEntries": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_MAP_ENTRIES,
                        "description": "Maximum number of tree entries to return. Defaults to 200."
                    },
                    "includeFiles": {
                        "type": "boolean",
                        "description": "Whether tree output should include files. Defaults to true. Summary statistics still count files."
                    }
                }
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        let args: WorkspaceMapArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("workspace_map 参数无效：{error}")))?;
        let max_depth = args
            .max_depth
            .unwrap_or(DEFAULT_MAX_DEPTH)
            .clamp(1, MAX_MAP_DEPTH);
        let max_entries = args
            .max_entries
            .unwrap_or(DEFAULT_MAX_ENTRIES)
            .clamp(1, MAX_MAP_ENTRIES);
        let include_files = args.include_files.unwrap_or(true);

        let workspace_root = if args
            .focus_path
            .as_deref()
            .is_some_and(|path| !path.trim().is_empty())
        {
            None
        } else {
            context.workspace_root_optional()?
        };
        let focus_root = resolve_focus_root(
            context,
            workspace_root.as_deref(),
            args.focus_path.as_deref(),
        )?;
        let cancellation_token = context.cancellation_token();
        let focus_path =
            context.display_path(args.focus_path.as_deref().unwrap_or("."), &focus_root)?;
        let focus_path = if focus_path.is_empty() {
            ".".to_string()
        } else {
            focus_path
        };
        let resolver = context.workspace_resolver();
        let containing_root = resolver
            .containing_root(&focus_root)
            .map_err(AgentError::new)?;
        let display_root = containing_root.as_deref().unwrap_or(focus_root.as_path());

        let walk = walk_workspace_with_cancellation(&focus_root, &cancellation_token)?;
        context.validate_folder_path(args.focus_path.as_deref().unwrap_or("."))?;
        let mut summary = build_summary(
            display_root,
            &focus_root,
            &walk.entries,
            &cancellation_token,
        )?;
        let mut tree = build_tree(
            display_root,
            &focus_root,
            &walk.entries,
            max_depth,
            max_entries,
            include_files,
            &cancellation_token,
        )?;
        // Folder references are external to the workspace resolver. Keep their opaque namespace
        // on every returned path so the model can feed a directory entry back to read_file.
        let folder_namespace = args
            .focus_path
            .as_deref()
            .and_then(|path| ResourceLocator::parse(path).ok())
            .and_then(|locator| match locator {
                ResourceLocator::Folder(value) => Some(value),
                _ => None,
            });
        if let Some(namespace) = folder_namespace.as_deref() {
            qualify_folder_map_paths(&mut summary, namespace);
            for entry in &mut tree.entries {
                qualify_folder_map_paths(entry, namespace);
            }
        }
        if folder_namespace.is_none() && containing_root.is_some() {
            qualify_workspace_map_paths(&mut summary, &resolver, display_root);
            for entry in &mut tree.entries {
                qualify_workspace_map_paths(entry, &resolver, display_root);
            }
        }

        let walk_partial = walk.truncated;
        let tree_partial = tree.omitted > 0;
        let refine = (walk_partial || tree_partial).then_some(
            "Narrow focusPath or reduce maxDepth, then call workspace_map again for a more focused summary.",
        );

        Ok(json!({
            "workspace": {
                "focusPath": focus_path,
                "maxDepth": max_depth,
                "maxEntries": max_entries,
                "includeFiles": include_files
            },
            "summary": summary,
            "tree": tree.entries,
            "treeText": tree.text,
            "coverage": {
                "walk": {
                    "scanned": walk.entries.len(),
                    "limit": super::MAX_WALK_ENTRIES,
                    "partial": walk_partial,
                    "stopReason": walk_partial.then_some("walk_entry_limit")
                },
                "tree": {
                    "total": tree.total,
                    "returned": tree.returned,
                    "omitted": tree.omitted,
                    "partial": tree_partial,
                    "stopReason": tree_partial.then_some("depth_or_entry_limit")
                }
            },
            "refine": refine,
            "truncated": {
                "walk": walk_partial,
                "tree": tree_partial
            }
        }))
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        let projected = result.result.as_ref().and_then(|value| {
            super::model_projection::retain_object_fields(
                value,
                &["summary", "treeText", "coverage", "refine", "truncated"],
            )
        });
        super::model_projection::compact_model_result(result, projected)
    }
}

fn qualify_folder_map_paths(value: &mut Value, namespace: &str) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if key == "path" {
                    if let Some(path) = value.as_str() {
                        let suffix = path.trim_matches('/');
                        *value = if suffix.is_empty() || suffix == "." {
                            json!(namespace)
                        } else {
                            json!(format!("{namespace}/{suffix}"))
                        };
                    }
                } else {
                    qualify_folder_map_paths(value, namespace);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                qualify_folder_map_paths(value, namespace);
            }
        }
        _ => {}
    }
}

fn qualify_workspace_map_paths(
    value: &mut Value,
    resolver: &crate::workspace::WorkspaceResolver,
    root: &Path,
) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if key == "path" {
                    if let Some(path) = value.as_str() {
                        *value = json!(resolver.display_path(&root.join(path)));
                    }
                } else {
                    qualify_workspace_map_paths(value, resolver, root);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                qualify_workspace_map_paths(value, resolver, root);
            }
        }
        _ => {}
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceMapArgs {
    focus_path: Option<String>,
    max_depth: Option<usize>,
    max_entries: Option<usize>,
    include_files: Option<bool>,
}

struct TreeOutput {
    entries: Vec<Value>,
    text: String,
    total: usize,
    returned: usize,
    omitted: usize,
}

#[derive(Default)]
struct LanguageStat {
    files: usize,
    bytes: u64,
    extensions: BTreeSet<String>,
}

#[derive(Default)]
struct DirectoryStat {
    files: usize,
    directories: usize,
    bytes: u64,
}

#[derive(Clone)]
struct FileCandidate {
    path: String,
    size_bytes: u64,
    kind: Option<&'static str>,
    reason: Option<&'static str>,
}

struct BoundedCollection {
    items: Vec<Value>,
    total: usize,
}

impl BoundedCollection {
    fn coverage(&self) -> Value {
        let omitted = self.total.saturating_sub(self.items.len());
        json!({
            "returned": self.items.len(),
            "total": self.total,
            "omitted": omitted,
            "truncated": omitted > 0
        })
    }
}

fn resolve_focus_root(
    context: &ToolExecutionContext,
    workspace_root: Option<&Path>,
    focus_path: Option<&str>,
) -> AgentResult<PathBuf> {
    let Some(focus_path) = focus_path.map(str::trim).filter(|path| !path.is_empty()) else {
        return workspace_root.map(Path::to_path_buf).ok_or_else(|| {
            AgentError::new(
                "当前没有 workspace；请使用 focusPath 指定绝对目录或 @desktop、@documents、@downloads、@home。",
            )
        });
    };

    let focus_root = context.resolve_existing_path(focus_path)?;
    if !focus_root.is_dir() {
        return Err(AgentError::new(
            "workspace_map.focusPath 必须是可访问的目录。",
        ));
    }

    Ok(focus_root)
}

fn build_summary(
    workspace_root: &Path,
    focus_root: &Path,
    entries: &[super::WalkEntry],
    cancellation_token: &AgentCancellationToken,
) -> AgentResult<Value> {
    let mut file_count = 0usize;
    let mut directory_count = 0usize;
    let mut total_file_bytes = 0u64;
    let mut language_stats = BTreeMap::<String, LanguageStat>::new();
    let mut directory_stats = BTreeMap::<String, DirectoryStat>::new();
    let mut important_files = Vec::<FileCandidate>::new();
    let mut entrypoint_candidates = Vec::<FileCandidate>::new();
    let mut test_candidates = Vec::<FileCandidate>::new();
    let mut documentation_files = Vec::<FileCandidate>::new();

    for entry in entries {
        cancellation_token.check()?;
        if entry.is_dir {
            directory_count += 1;
            bump_top_directory(
                workspace_root,
                focus_root,
                &entry.path,
                entry.is_dir,
                entry.size_bytes,
                &mut directory_stats,
            );
            continue;
        }

        file_count += 1;
        total_file_bytes = total_file_bytes.saturating_add(entry.size_bytes);
        bump_language_stat(&entry.path, entry.size_bytes, &mut language_stats);
        bump_top_directory(
            workspace_root,
            focus_root,
            &entry.path,
            entry.is_dir,
            entry.size_bytes,
            &mut directory_stats,
        );

        let display_path = relative_display(workspace_root, &entry.path);
        if let Some(kind) = important_file_kind(&display_path, &entry.path) {
            important_files.push(FileCandidate {
                path: display_path.clone(),
                size_bytes: entry.size_bytes,
                kind: Some(kind),
                reason: None,
            });
        }
        if let Some(reason) = entrypoint_reason(&display_path, &entry.path) {
            entrypoint_candidates.push(FileCandidate {
                path: display_path.clone(),
                size_bytes: entry.size_bytes,
                kind: None,
                reason: Some(reason),
            });
        }
        if let Some(reason) = test_reason(&display_path, &entry.path) {
            test_candidates.push(FileCandidate {
                path: display_path.clone(),
                size_bytes: entry.size_bytes,
                kind: None,
                reason: Some(reason),
            });
        }
        if let Some(reason) = documentation_reason(&display_path, &entry.path) {
            documentation_files.push(FileCandidate {
                path: display_path,
                size_bytes: entry.size_bytes,
                kind: None,
                reason: Some(reason),
            });
        }
    }

    let languages = language_stats_json(language_stats);
    let top_directories = top_directories_json(directory_stats);
    let important_files = candidates_json(important_files, MAX_IMPORTANT_FILES);
    let entrypoint_candidates = candidates_json(entrypoint_candidates, MAX_CANDIDATES);
    let test_candidates = candidates_json(test_candidates, MAX_CANDIDATES);
    let documentation_files = candidates_json(documentation_files, MAX_CANDIDATES);
    let collection_coverage = json!({
        "languages": languages.coverage(),
        "topDirectories": top_directories.coverage(),
        "importantFiles": important_files.coverage(),
        "entrypointCandidates": entrypoint_candidates.coverage(),
        "testCandidates": test_candidates.coverage(),
        "documentationFiles": documentation_files.coverage()
    });

    Ok(json!({
        "fileCount": file_count,
        "directoryCount": directory_count,
        "totalFileBytes": total_file_bytes,
        "languages": languages.items,
        "topDirectories": top_directories.items,
        "importantFiles": important_files.items,
        "entrypointCandidates": entrypoint_candidates.items,
        "testCandidates": test_candidates.items,
        "documentationFiles": documentation_files.items,
        "collectionCoverage": collection_coverage
    }))
}

fn build_tree(
    workspace_root: &Path,
    focus_root: &Path,
    entries: &[super::WalkEntry],
    max_depth: usize,
    max_entries: usize,
    include_files: bool,
    cancellation_token: &AgentCancellationToken,
) -> AgentResult<TreeOutput> {
    let mut output_entries = Vec::new();
    let mut lines = Vec::new();
    let mut total = 0usize;

    for entry in entries {
        cancellation_token.check()?;
        if !include_files && !entry.is_dir {
            continue;
        }

        let depth = depth_relative_to(focus_root, &entry.path);
        if depth == 0 {
            continue;
        }
        total = total.saturating_add(1);
        if depth > max_depth {
            continue;
        }
        if output_entries.len() >= max_entries {
            continue;
        }

        let path = relative_display(workspace_root, &entry.path);
        let kind = if entry.is_dir { "directory" } else { "file" };
        let name = entry
            .path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());

        output_entries.push(json!({
            "path": path,
            "name": name,
            "kind": kind,
            "depth": depth,
            "sizeBytes": if entry.is_dir { Value::Null } else { json!(entry.size_bytes) }
        }));

        let indent = "  ".repeat(depth.saturating_sub(1));
        let suffix = if entry.is_dir { "/" } else { "" };
        lines.push(format!("{indent}{name}{suffix}"));
    }

    let returned = output_entries.len();
    Ok(TreeOutput {
        entries: output_entries,
        text: lines.join("\n"),
        total,
        returned,
        omitted: total.saturating_sub(returned),
    })
}

fn bump_language_stat(
    path: &Path,
    size_bytes: u64,
    language_stats: &mut BTreeMap<String, LanguageStat>,
) {
    let language = language_for_path(path);
    let extension = extension_for_path(path);
    let stat = language_stats.entry(language).or_default();
    stat.files += 1;
    stat.bytes = stat.bytes.saturating_add(size_bytes);
    if let Some(extension) = extension {
        stat.extensions.insert(extension);
    }
}

fn bump_top_directory(
    workspace_root: &Path,
    focus_root: &Path,
    path: &Path,
    is_dir: bool,
    size_bytes: u64,
    directory_stats: &mut BTreeMap<String, DirectoryStat>,
) {
    let relative_to_focus = path.strip_prefix(focus_root).unwrap_or(path);
    let Some(first_component) = relative_to_focus.components().next() else {
        return;
    };
    let first_component = first_component.as_os_str().to_string_lossy();
    if first_component.is_empty() {
        return;
    }

    let directory_path = focus_root.join(first_component.as_ref());
    let display_path = relative_display(workspace_root, &directory_path);
    let stat = directory_stats.entry(display_path).or_default();
    if is_dir {
        stat.directories += 1;
    } else {
        stat.files += 1;
        stat.bytes = stat.bytes.saturating_add(size_bytes);
    }
}

fn language_stats_json(language_stats: BTreeMap<String, LanguageStat>) -> BoundedCollection {
    let mut stats = language_stats.into_iter().collect::<Vec<_>>();
    stats.sort_by_key(|(name, stat)| (Reverse(stat.files), Reverse(stat.bytes), name.clone()));
    let total = stats.len();

    let items = stats
        .into_iter()
        .take(MAX_LANGUAGE_STATS)
        .map(|(language, stat)| {
            json!({
                "language": language,
                "files": stat.files,
                "bytes": stat.bytes,
                "extensions": stat.extensions.into_iter().collect::<Vec<_>>()
            })
        })
        .collect();

    BoundedCollection { items, total }
}

fn top_directories_json(directory_stats: BTreeMap<String, DirectoryStat>) -> BoundedCollection {
    let mut stats = directory_stats.into_iter().collect::<Vec<_>>();
    stats.sort_by_key(|(path, stat)| {
        (
            Reverse(stat.files + stat.directories),
            Reverse(stat.bytes),
            path.clone(),
        )
    });
    let total = stats.len();

    let items = stats
        .into_iter()
        .take(MAX_TOP_DIRECTORIES)
        .map(|(path, stat)| {
            json!({
                "path": if path.is_empty() { ".".to_string() } else { path },
                "files": stat.files,
                "directories": stat.directories,
                "bytes": stat.bytes
            })
        })
        .collect();

    BoundedCollection { items, total }
}

fn candidates_json(mut candidates: Vec<FileCandidate>, limit: usize) -> BoundedCollection {
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    let total = candidates.len();
    let items = candidates
        .into_iter()
        .take(limit)
        .map(|candidate| {
            let mut value = json!({
                "path": candidate.path,
                "sizeBytes": candidate.size_bytes
            });
            if let Some(kind) = candidate.kind {
                value["kind"] = json!(kind);
            }
            if let Some(reason) = candidate.reason {
                value["reason"] = json!(reason);
            }
            value
        })
        .collect();

    BoundedCollection { items, total }
}

fn depth_relative_to(root: &Path, path: &Path) -> usize {
    path.strip_prefix(root).unwrap_or(path).components().count()
}

fn important_file_kind(display_path: &str, path: &Path) -> Option<&'static str> {
    let name = file_name_lower(path);
    let path = display_path.to_ascii_lowercase();

    if is_documentation_name(&name) {
        return Some("documentation");
    }
    if matches!(
        name.as_str(),
        "package.json"
            | "pnpm-workspace.yaml"
            | "yarn.lock"
            | "package-lock.json"
            | "bun.lockb"
            | "cargo.toml"
            | "cargo.lock"
            | "pyproject.toml"
            | "requirements.txt"
            | "uv.lock"
            | "go.mod"
            | "go.sum"
            | "pom.xml"
            | "build.gradle"
            | "settings.gradle"
            | "dockerfile"
            | "docker-compose.yml"
            | "docker-compose.yaml"
            | "makefile"
            | "tsconfig.json"
            | "vite.config.ts"
            | "vite.config.js"
            | "next.config.js"
            | "next.config.mjs"
            | "eslint.config.js"
            | "prettier.config.js"
            | "tailwind.config.js"
            | "tailwind.config.ts"
    ) {
        return Some("config");
    }
    if path.ends_with("/src/main.rs")
        || path.ends_with("/src/lib.rs")
        || path.ends_with("/src/main.ts")
        || path.ends_with("/src/main.tsx")
        || path.ends_with("/src/index.ts")
        || path.ends_with("/src/index.tsx")
        || path.ends_with("/src/app.tsx")
        || path.ends_with("/main.py")
    {
        return Some("entrypoint");
    }

    None
}

fn entrypoint_reason(display_path: &str, path: &Path) -> Option<&'static str> {
    let name = file_name_lower(path);
    let path = display_path.to_ascii_lowercase();
    if matches!(
        name.as_str(),
        "main.rs"
            | "lib.rs"
            | "main.ts"
            | "main.tsx"
            | "main.js"
            | "main.jsx"
            | "index.ts"
            | "index.tsx"
            | "index.js"
            | "index.jsx"
            | "app.tsx"
            | "app.jsx"
            | "main.py"
            | "app.py"
            | "server.py"
            | "main.go"
            | "main.java"
    ) {
        return Some("common application or library entry file name");
    }
    if path.contains("/src/bin/") {
        return Some("Rust binary entrypoint directory");
    }
    None
}

fn test_reason(display_path: &str, path: &Path) -> Option<&'static str> {
    let name = file_name_lower(path);
    let path = display_path.to_ascii_lowercase();
    if path.contains("/__tests__/") || path.contains("/tests/") {
        return Some("test directory");
    }
    if name.contains(".test.") || name.contains(".spec.") || name.ends_with("_test.go") {
        return Some("test file naming pattern");
    }
    None
}

fn documentation_reason(display_path: &str, path: &Path) -> Option<&'static str> {
    let name = file_name_lower(path);
    if is_documentation_name(&name) {
        return Some("documentation file name");
    }
    let path = display_path.to_ascii_lowercase();
    if path.starts_with("docs/") || path.contains("/docs/") {
        return Some("docs directory");
    }
    None
}

fn is_documentation_name(name: &str) -> bool {
    name == "readme"
        || name.starts_with("readme.")
        || name.starts_with("changelog.")
        || name.starts_with("contributing.")
        || name.starts_with("license.")
        || name == "license"
}

fn language_for_path(path: &Path) -> String {
    let name = file_name_lower(path);
    if matches!(name.as_str(), "dockerfile" | "containerfile") {
        return "Dockerfile".to_string();
    }
    if matches!(name.as_str(), "makefile" | "gnumakefile") {
        return "Makefile".to_string();
    }

    match extension_for_path(path).as_deref() {
        Some("rs") => "Rust",
        Some("ts") | Some("tsx") | Some("mts") | Some("cts") => "TypeScript",
        Some("js") | Some("jsx") | Some("mjs") | Some("cjs") => "JavaScript",
        Some("py") | Some("pyi") | Some("ipynb") => "Python",
        Some("go") => "Go",
        Some("java") => "Java",
        Some("kt") | Some("kts") => "Kotlin",
        Some("swift") => "Swift",
        Some("c") | Some("h") => "C/C++",
        Some("cc") | Some("cpp") | Some("cxx") | Some("hpp") | Some("hh") | Some("hxx") => "C/C++",
        Some("cs") => "C#",
        Some("rb") => "Ruby",
        Some("php") => "PHP",
        Some("sh") | Some("bash") | Some("zsh") | Some("fish") => "Shell",
        Some("ps1") => "PowerShell",
        Some("html") | Some("htm") => "HTML",
        Some("css") | Some("scss") | Some("sass") | Some("less") => "CSS",
        Some("vue") => "Vue",
        Some("svelte") => "Svelte",
        Some("astro") => "Astro",
        Some("json") | Some("jsonl") => "JSON",
        Some("yaml") | Some("yml") => "YAML",
        Some("toml") => "TOML",
        Some("xml") | Some("svg") => "XML",
        Some("md") | Some("markdown") | Some("mdx") | Some("rst") => "Documentation",
        Some("sql") => "SQL",
        Some("graphql") | Some("gql") => "GraphQL",
        Some("proto") => "Protocol Buffers",
        Some("prisma") => "Prisma",
        Some("tf") | Some("tfvars") | Some("hcl") => "HCL/Terraform",
        Some("csv") | Some("tsv") | Some("xlsx") | Some("xls") => "Spreadsheet",
        Some("pdf") => "PDF",
        Some("doc") | Some("docx") => "Word",
        Some("ppt") | Some("pptx") => "Presentation",
        Some("lock") => "Lockfile",
        Some(extension) if !extension.is_empty() => "Other",
        _ => "Other",
    }
    .to_string()
}

fn extension_for_path(path: &Path) -> Option<String> {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .filter(|extension| !extension.is_empty())
}

fn file_name_lower(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::{ToolExecutionContext, ToolRegistry, WalkEntry};
    use super::{
        build_summary, MAX_CANDIDATES, MAX_IMPORTANT_FILES, MAX_LANGUAGE_STATS, MAX_TOP_DIRECTORIES,
    };
    use crate::cancellation::AgentCancellationToken;
    use crate::protocol::{
        AgentApprovalStatus, AgentRunContext, AgentToolCall, AgentWorkspaceContext,
    };
    use serde_json::json;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn workspace_map_summarizes_project_structure() {
        let fixture = TestWorkspace::new();
        fixture.write_file("package.json", "{}");
        fixture.write_file("README.md", "# Demo");
        fixture.write_file("src/main.tsx", "export function main() {}");
        fixture.write_file("src/App.test.tsx", "test('ok', () => {})");
        fixture.write_file("node_modules/ignored/index.js", "ignored");
        let context = fixture.context();
        let registry = ToolRegistry::defaults_with_search(None);

        let result = registry.execute(
            &context,
            &AgentToolCall {
                id: "call-1".to_string(),
                tool: "workspace_map".to_string(),
                args: json!({ "maxDepth": 3, "maxEntries": 20 }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );

        assert!(result.ok, "{:?}", result.error);
        let value = result.result.as_ref().unwrap();
        assert_eq!(value["summary"]["fileCount"], 4);
        assert!(value["treeText"].as_str().unwrap().contains("src/"));
        assert!(value["treeText"].as_str().unwrap().contains("main.tsx"));
        assert!(!value["treeText"].as_str().unwrap().contains("node_modules"));
        assert_eq!(
            value["summary"]["entrypointCandidates"][0]["path"],
            "src/main.tsx"
        );
        assert_eq!(
            value["summary"]["testCandidates"][0]["path"],
            "src/App.test.tsx"
        );
        assert_eq!(value["coverage"]["walk"]["partial"], false);
        assert_eq!(value["coverage"]["tree"]["returned"], 5);
        assert_eq!(value["coverage"]["tree"]["total"], 5);
        assert_eq!(value["coverage"]["tree"]["omitted"], 0);
        assert!(value["refine"].is_null());

        let coverage = &value["summary"]["collectionCoverage"];
        for collection in [
            "languages",
            "topDirectories",
            "importantFiles",
            "entrypointCandidates",
            "testCandidates",
            "documentationFiles",
        ] {
            assert_eq!(
                coverage[collection]["returned"],
                coverage[collection]["total"]
            );
            assert_eq!(coverage[collection]["omitted"], 0);
            assert_eq!(coverage[collection]["truncated"], false);
        }
        assert!(!crate::tools::tool_result_truncated_at_source(&result));

        let model = registry.model_projection(&result);
        let model_value = model.result.as_ref().unwrap();
        assert!(model_value.get("workspace").is_none());
        assert_eq!(
            model_value["summary"]["collectionCoverage"],
            value["summary"]["collectionCoverage"]
        );
        assert_eq!(
            model_value["coverage"]["walk"]["scanned"],
            value["coverage"]["walk"]["scanned"]
        );
        assert_eq!(
            model_value["coverage"]["walk"]["partial"],
            value["coverage"]["walk"]["partial"]
        );
        assert_eq!(
            model_value["coverage"]["tree"]["total"],
            value["coverage"]["tree"]["total"]
        );
        assert_eq!(
            model_value["coverage"]["tree"]["returned"],
            value["coverage"]["tree"]["returned"]
        );
        assert_eq!(
            model_value["coverage"]["tree"]["omitted"],
            value["coverage"]["tree"]["omitted"]
        );
        assert!(model_value["coverage"]["walk"].get("stopReason").is_none());
        assert!(model_value["coverage"]["tree"].get("stopReason").is_none());

        let event = registry.event_projection(&result);
        assert_eq!(
            event.result.as_ref().unwrap()["summary"]["collectionCoverage"],
            value["summary"]["collectionCoverage"]
        );
        assert!(event.result.as_ref().unwrap().get("workspace").is_some());

        let trace = registry.trace_projection(&result);
        assert_eq!(
            trace.result.as_ref().unwrap()["summary"]["collectionCoverage"],
            value["summary"]["collectionCoverage"]
        );
    }

    #[test]
    fn workspace_map_can_focus_on_subdirectory() {
        let fixture = TestWorkspace::new();
        fixture.write_file("agent/rust/src/lib.rs", "pub fn lib() {}");
        fixture.write_file("apps/desktop/src/main.tsx", "main()");
        let context = fixture.context();
        let registry = ToolRegistry::defaults_with_search(None);

        let result = registry.execute(
            &context,
            &AgentToolCall {
                id: "call-1".to_string(),
                tool: "workspace_map".to_string(),
                args: json!({ "focusPath": "agent", "maxDepth": 4 }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );

        assert!(result.ok, "{:?}", result.error);
        let value = result.result.as_ref().unwrap();
        assert_eq!(value["workspace"]["focusPath"], "agent");
        assert!(value["treeText"].as_str().unwrap().contains("rust/"));
        assert!(!value["treeText"].as_str().unwrap().contains("desktop"));
    }

    #[test]
    fn workspace_map_tree_limit_is_explicit_and_recommends_refinement() {
        let fixture = TestWorkspace::new();
        fixture.write_file("src/a.rs", "a");
        fixture.write_file("src/b.rs", "b");
        fixture.write_file("src/nested/c.rs", "c");
        let context = fixture.context();
        let registry = ToolRegistry::defaults_with_search(None);

        let result = registry.execute(
            &context,
            &AgentToolCall {
                id: "call-tree-coverage".to_string(),
                tool: "workspace_map".to_string(),
                args: json!({ "maxDepth": 1, "maxEntries": 1 }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );

        assert!(result.ok, "{:?}", result.error);
        let value = result.result.as_ref().unwrap();
        assert_eq!(value["coverage"]["tree"]["returned"], 1);
        assert_eq!(value["coverage"]["tree"]["total"], 5);
        assert_eq!(value["coverage"]["tree"]["omitted"], 4);
        assert_eq!(value["coverage"]["tree"]["partial"], true);
        assert_eq!(
            value["coverage"]["tree"]["stopReason"],
            "depth_or_entry_limit"
        );
        assert!(value["refine"]
            .as_str()
            .unwrap()
            .contains("Narrow focusPath"));
        assert_eq!(value["truncated"]["tree"], true);
    }

    #[test]
    fn workspace_map_reports_coverage_for_every_bounded_summary_collection() {
        let workspace_root = Path::new("/workspace");
        let mut entries = Vec::new();

        for index in 0..(MAX_IMPORTANT_FILES + 5) {
            entries.push(file_entry(format!(
                "/workspace/config-{index}/package.json"
            )));
        }
        for index in 0..(MAX_CANDIDATES + 5) {
            entries.push(file_entry(format!(
                "/workspace/entrypoints/{index}/main.go"
            )));
            entries.push(file_entry(format!("/workspace/tests/case-{index}.test.ts")));
            entries.push(file_entry(format!("/workspace/docs/page-{index}.md")));
        }
        for (index, extension) in [
            "rs", "ts", "js", "py", "go", "java", "kt", "swift", "c", "cs", "rb", "php", "sh",
            "ps1", "html", "css", "vue", "svelte", "astro", "json", "yaml", "toml", "xml", "md",
            "sql", "gql", "proto", "prisma", "tf", "csv", "pdf", "doc", "ppt", "lock", "bin",
        ]
        .into_iter()
        .enumerate()
        {
            entries.push(file_entry(format!(
                "/workspace/languages/sample-{index}.{extension}"
            )));
        }

        let summary = build_summary(
            workspace_root,
            workspace_root,
            &entries,
            &AgentCancellationToken::new(),
        )
        .unwrap();
        let coverage = &summary["collectionCoverage"];

        assert_eq!(
            coverage["importantFiles"],
            json!({
                "returned": MAX_IMPORTANT_FILES,
                "total": MAX_IMPORTANT_FILES + 5,
                "omitted": 5,
                "truncated": true
            })
        );
        for collection in [
            "entrypointCandidates",
            "testCandidates",
            "documentationFiles",
        ] {
            assert_eq!(
                coverage[collection],
                json!({
                    "returned": MAX_CANDIDATES,
                    "total": MAX_CANDIDATES + 5,
                    "omitted": 5,
                    "truncated": true
                })
            );
        }

        assert_eq!(
            coverage["topDirectories"],
            json!({
                "returned": MAX_TOP_DIRECTORIES,
                "total": MAX_IMPORTANT_FILES + 9,
                "omitted": MAX_IMPORTANT_FILES + 9 - MAX_TOP_DIRECTORIES,
                "truncated": true
            })
        );

        let language_total = coverage["languages"]["total"].as_u64().unwrap() as usize;
        assert!(language_total > MAX_LANGUAGE_STATS);
        assert_eq!(coverage["languages"]["returned"], json!(MAX_LANGUAGE_STATS));
        assert_eq!(
            coverage["languages"]["omitted"],
            json!(language_total - MAX_LANGUAGE_STATS)
        );
        assert_eq!(coverage["languages"]["truncated"], true);

        let raw = crate::protocol::AgentToolResult {
            exact_archive_file: None,
            call_id: "call-workspace-map-coverage".to_string(),
            tool: "workspace_map".to_string(),
            ok: true,
            result: Some(json!({ "summary": summary })),
            error: None,
        };
        assert!(crate::tools::tool_result_truncated_at_source(&raw));
    }

    fn file_entry(path: String) -> WalkEntry {
        WalkEntry {
            path: PathBuf::from(path),
            is_dir: false,
            size_bytes: 1,
        }
    }

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = TEST_WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("my-copilot-agent-test-workspace-map-{unique}"));
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
