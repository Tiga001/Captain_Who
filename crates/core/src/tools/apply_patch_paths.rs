// Path permission checks and supported text-file filtering for apply_patch.
use super::clean_relative_path;
use crate::protocol::{AgentError, AgentResult, AgentWritePermission};
use crate::system_paths::expand_system_path;
use std::path::Path;

const TEXT_PATCH_EXTENSIONS: &[&str] = &[
    "astro",
    "bash",
    "bat",
    "bib",
    "c",
    "cc",
    "cfg",
    "cjs",
    "clj",
    "cljc",
    "cljs",
    "cmd",
    "cmake",
    "conf",
    "cpp",
    "cs",
    "css",
    "csv",
    "cxx",
    "dart",
    "dockerfile",
    "d.ts",
    "edn",
    "elm",
    "env",
    "erl",
    "ex",
    "exs",
    "fish",
    "fs",
    "fsi",
    "fsx",
    "gitattributes",
    "gitignore",
    "gql",
    "gradle",
    "graphql",
    "groovy",
    "go",
    "h",
    "hcl",
    "hh",
    "hrl",
    "hs",
    "htm",
    "html",
    "hxx",
    "hpp",
    "ini",
    "ipynb",
    "java",
    "jl",
    "js",
    "json",
    "jsonl",
    "jsx",
    "kt",
    "kts",
    "less",
    "lhs",
    "lock",
    "log",
    "lua",
    "m",
    "make",
    "markdown",
    "md",
    "mdx",
    "mk",
    "ml",
    "mli",
    "mjs",
    "nim",
    "nims",
    "php",
    "pl",
    "plist",
    "pm",
    "prisma",
    "properties",
    "proto",
    "ps1",
    "py",
    "pyi",
    "r",
    "rb",
    "rc",
    "rs",
    "rst",
    "sass",
    "scala",
    "scss",
    "sh",
    "sol",
    "sql",
    "sv",
    "svelte",
    "svg",
    "svh",
    "swift",
    "tex",
    "text",
    "tf",
    "tfvars",
    "toml",
    "ts",
    "tsx",
    "tsv",
    "txt",
    "v",
    "vh",
    "vue",
    "xml",
    "yaml",
    "yml",
    "zig",
    "zsh",
];

const TEXT_PATCH_BASENAMES: &[&str] = &[
    ".dockerignore",
    ".editorconfig",
    ".env",
    ".gitattributes",
    ".gitignore",
    ".npmrc",
    ".prettierrc",
    ".stylelintrc",
    "Brewfile",
    "CMakeLists.txt",
    "Dockerfile",
    "Gemfile",
    "Justfile",
    "Makefile",
    "Podfile",
    "Rakefile",
];

const UNSUPPORTED_DOCUMENT_EXTENSIONS: &[&str] =
    &["pdf", "doc", "docx", "ppt", "pptx", "xls", "xlsx"];

pub(crate) fn sanitize_file_path(
    path: &str,
    permission: AgentWritePermission,
) -> AgentResult<String> {
    let path = path.trim();
    if path.is_empty() {
        return Err(AgentError::new("apply_patch.filePath 不能为空。"));
    }
    if permission == AgentWritePermission::Denied {
        return Err(AgentError::new("当前写入权限为 denied，不能提出文件修改。"));
    }
    if let Some(expanded) = expand_system_path(path).map_err(AgentError::new)? {
        if permission != AgentWritePermission::All {
            return Err(AgentError::new(
                "写入系统路径别名需要将写入范围设为“所有位置”。",
            ));
        }
        let expanded = expanded.to_string_lossy().to_string();
        validate_text_patch_path(&expanded)?;
        return Ok(expanded);
    }
    if Path::new(path).is_absolute() {
        if permission != AgentWritePermission::All {
            return Err(AgentError::new("当前写入权限仅允许修改 workspace 内文件。"));
        }
        validate_text_patch_path(path)?;
        return Ok(Path::new(path).to_string_lossy().to_string());
    }

    let cleaned = clean_relative_path(path)?;
    let normalized = cleaned
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");

    validate_text_patch_path(&normalized)?;

    Ok(normalized)
}

pub(crate) fn validate_text_patch_path(path: &str) -> AgentResult<()> {
    let file_name = Path::new(path)
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .unwrap_or_default();

    if TEXT_PATCH_BASENAMES
        .iter()
        .any(|basename| file_name == *basename)
    {
        return Ok(());
    }

    let lower_name = file_name.to_ascii_lowercase();
    if TEXT_PATCH_BASENAMES
        .iter()
        .any(|basename| lower_name == basename.to_ascii_lowercase())
    {
        return Ok(());
    }

    if lower_name.ends_with(".d.ts") {
        return Ok(());
    }

    let extension = Path::new(&lower_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if UNSUPPORTED_DOCUMENT_EXTENSIONS
        .iter()
        .any(|unsupported| extension == *unsupported)
    {
        return Err(AgentError::new(format!(
            "apply_patch 不支持直接修改 .{extension} 文档。PDF/Office 文件需要专用编辑工具。"
        )));
    }
    if TEXT_PATCH_EXTENSIONS
        .iter()
        .any(|allowed| extension == *allowed)
    {
        return Ok(());
    }

    Err(AgentError::new(format!(
        "apply_patch 暂不支持该文件类型：{path}。当前只支持文本、代码、配置、CSV/TSV、Markdown、JSON/YAML/TOML/XML/SVG/IPYNB 等可 diff 文件。"
    )))
}
