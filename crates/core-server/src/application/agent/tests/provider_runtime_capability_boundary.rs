use std::path::{Path, PathBuf};

const ALLOWED_PROVIDER_BOUNDARIES: &[&str] = &[
    "crates/core/src/provider_profile.rs",
    "crates/core/src/provider_registration.rs",
    "crates/core/src/llm/adapter.rs",
    "crates/core/src/llm/payload.rs",
    "crates/core/src/llm/stream.rs",
];

const FORBIDDEN_GENERIC_PROVIDER_MARKERS: &[&str] = &[
    "ProviderProfileId::",
    "DeepSeek",
    "deepseek",
    "is_special_provider",
    "is_deepseek",
];

#[test]
fn vendor_identity_is_confined_to_registered_provider_boundaries() {
    let core_server_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = core_server_root
        .parent()
        .and_then(Path::parent)
        .expect("core-server crate must live under the workspace crates directory");
    let mut rust_sources = Vec::new();
    collect_rust_sources(&workspace_root.join("crates/core/src"), &mut rust_sources);
    collect_rust_sources(
        &workspace_root.join("crates/core-server/src"),
        &mut rust_sources,
    );

    let mut violations = Vec::new();
    for path in rust_sources {
        let relative = path
            .strip_prefix(workspace_root)
            .expect("source path must remain inside the workspace")
            .to_string_lossy()
            .replace('\\', "/");
        if is_test_source(&relative) || ALLOWED_PROVIDER_BOUNDARIES.contains(&relative.as_str()) {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"));
        let production = production_prefix(&source);
        for (index, line) in production.lines().enumerate() {
            if let Some(marker) = FORBIDDEN_GENERIC_PROVIDER_MARKERS
                .iter()
                .find(|marker| line.contains(**marker))
            {
                violations.push(format!("{relative}:{} contains {marker}", index + 1));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Provider vendor identity escaped its Profile/Registration/Adapter boundary:\n{}\n\
         Add runtime behavior through code-owned Provider capabilities instead of expanding the allowlist.",
        violations.join("\n")
    );
}

fn collect_rust_sources(root: &Path, output: &mut Vec<PathBuf>) {
    let mut entries = std::fs::read_dir(root)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", root.display()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("failed to enumerate {}: {error}", root.display()));
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_sources(&path, output);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            output.push(path);
        }
    }
}

fn is_test_source(relative: &str) -> bool {
    relative.contains("/tests/") || relative.ends_with("/tests.rs")
}

fn production_prefix(source: &str) -> &str {
    source
        .find("\n#[cfg(test)]\nmod ")
        .map_or(source, |index| &source[..index])
}
