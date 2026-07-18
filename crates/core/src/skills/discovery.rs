use super::model::{
    SkillCatalog, SkillDescriptor, SkillDiagnostic, SkillDiagnosticCode, SkillDiagnosticSeverity,
    SkillDiscoveryError, SkillScope,
};
use super::parser::parse_skill_document;
use super::workspace::{
    load_workspace_skill, percent_encode, resolve_workspace_skills_root, skill_revision,
    workspace_skill_id, ByteBudget, ScanBudget, WorkspaceRootError, WorkspaceSkillsRoot,
    MAX_SKILL_CATALOG_BYTES, MAX_SKILL_ROOT_ENTRIES, MAX_SKILL_SCAN_ENTRIES,
};
use crate::content_revision;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(test)]
use super::workspace::{
    find_exact_skill_file, normalize_windows_path_units, read_bounded_verified,
    read_open_file_bounded, BoundedReadError, ExactSkillFile, AGENTS_DIRECTORY,
    MAX_SKILL_FILE_BYTES, SKILLS_DIRECTORY, SKILL_FILE_NAME,
};
#[cfg(test)]
use std::fs::File;

#[derive(Debug, Default)]
pub struct SkillsService;

impl SkillsService {
    pub fn new() -> Self {
        Self
    }

    pub fn list_workspace(
        &self,
        workspace_id: &str,
        workspace_root: &Path,
    ) -> Result<SkillCatalog, SkillDiscoveryError> {
        let roots = match resolve_workspace_skills_root(workspace_root) {
            Ok(WorkspaceSkillsRoot::Missing) => {
                return Ok(finalize_catalog(Vec::new(), Vec::new(), false));
            }
            Ok(WorkspaceSkillsRoot::Ready(roots)) => roots,
            Err(WorkspaceRootError::Discovery(error)) => return Err(error),
            Err(WorkspaceRootError::Invalid(issue)) => {
                return Ok(catalog_with_root_diagnostic(
                    &issue.path,
                    issue.code,
                    issue.message,
                ));
            }
        };
        let canonical_skills_root = &roots.skills_root;

        let mut scan_budget = ScanBudget::new(MAX_SKILL_SCAN_ENTRIES);
        let (candidates, mut diagnostics, root_truncated) =
            collect_skill_directories(canonical_skills_root, &mut scan_budget);
        if root_truncated {
            return Ok(finalize_catalog(Vec::new(), diagnostics, true));
        }

        let mut skills = Vec::new();
        let mut byte_budget = ByteBudget::new(MAX_SKILL_CATALOG_BYTES);
        let mut catalog_truncated = false;
        for skill_directory in candidates {
            let loaded = match load_workspace_skill(
                &roots,
                &skill_directory,
                &mut scan_budget,
                &mut byte_budget,
            ) {
                Ok(loaded) => loaded,
                Err(issue) => {
                    let truncates_catalog = issue.truncates_catalog();
                    diagnostics.push(diagnostic(
                        &issue.path,
                        issue.code,
                        issue.severity,
                        issue.message,
                    ));
                    if truncates_catalog {
                        catalog_truncated = true;
                        break;
                    }
                    continue;
                }
            };
            let directory_name = loaded.directory_name.as_str();
            let canonical_skill_file = &loaded.canonical_path;
            let bytes = &loaded.bytes;

            if bytes.contains(&0) {
                diagnostics.push(diagnostic(
                    canonical_skill_file,
                    SkillDiagnosticCode::NulByte,
                    SkillDiagnosticSeverity::Error,
                    "SKILL.md contains a NUL byte.",
                ));
                continue;
            }
            let contents = match std::str::from_utf8(bytes) {
                Ok(contents) => contents,
                Err(_) => {
                    diagnostics.push(diagnostic(
                        canonical_skill_file,
                        SkillDiagnosticCode::InvalidUtf8,
                        SkillDiagnosticSeverity::Error,
                        "SKILL.md must be valid UTF-8.",
                    ));
                    continue;
                }
            };
            let document = match parse_skill_document(contents, directory_name) {
                Ok(document) => document,
                Err(error) => {
                    diagnostics.push(diagnostic(
                        canonical_skill_file,
                        error.diagnostic_code(),
                        SkillDiagnosticSeverity::Error,
                        error.to_string(),
                    ));
                    continue;
                }
            };
            let metadata = document.metadata;
            if metadata.name_was_defaulted {
                diagnostics.push(diagnostic(
                    canonical_skill_file,
                    SkillDiagnosticCode::DefaultedName,
                    SkillDiagnosticSeverity::Warning,
                    format!(
                        "Skill frontmatter has no name; using directory name `{directory_name}`."
                    ),
                ));
            }

            let Some(path) = canonical_skill_file.to_str() else {
                diagnostics.push(diagnostic(
                    canonical_skill_file,
                    SkillDiagnosticCode::UnsupportedPathEncoding,
                    SkillDiagnosticSeverity::Error,
                    "The canonical SKILL.md path cannot be represented as UTF-8.",
                ));
                continue;
            };
            skills.push(SkillDescriptor {
                id: workspace_skill_id(workspace_id, directory_name),
                name: metadata.name,
                description: metadata.description,
                scope: SkillScope::Workspace,
                path: path.to_owned(),
                relative_path: loaded.relative_path,
                revision: skill_revision(bytes),
            });
        }

        skills.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.id.cmp(&right.id))
        });
        append_duplicate_name_diagnostics(&skills, canonical_skills_root, &mut diagnostics);

        Ok(finalize_catalog(skills, diagnostics, catalog_truncated))
    }
}

fn collect_skill_directories(
    root: &Path,
    scan_budget: &mut ScanBudget,
) -> (Vec<PathBuf>, Vec<SkillDiagnostic>, bool) {
    let mut diagnostics = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            diagnostics.push(diagnostic(
                root,
                SkillDiagnosticCode::InvalidRoot,
                SkillDiagnosticSeverity::Error,
                format!("Cannot read workspace Skill root: {error}"),
            ));
            return (Vec::new(), diagnostics, false);
        }
    };

    let mut collected = Vec::new();
    let mut observed_entries = 0usize;
    for entry in entries {
        observed_entries = observed_entries.saturating_add(1);
        if !scan_budget.consume_entry() {
            diagnostics.push(diagnostic(
                root,
                SkillDiagnosticCode::ScanBudgetExceeded,
                SkillDiagnosticSeverity::Warning,
                format!(
                    "Workspace Skill discovery exceeded its {MAX_SKILL_SCAN_ENTRIES}-entry scan budget."
                ),
            ));
            return (Vec::new(), diagnostics, true);
        }
        if observed_entries > MAX_SKILL_ROOT_ENTRIES {
            diagnostics.push(diagnostic(
                root,
                SkillDiagnosticCode::TooManyEntries,
                SkillDiagnosticSeverity::Warning,
                format!(
                    "Workspace Skill root contains more than {MAX_SKILL_ROOT_ENTRIES} entries."
                ),
            ));
            return (Vec::new(), diagnostics, true);
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                diagnostics.push(diagnostic(
                    root,
                    SkillDiagnosticCode::UnreadableEntry,
                    SkillDiagnosticSeverity::Error,
                    format!("Cannot read an entry in the workspace Skill root: {error}"),
                ));
                continue;
            }
        };
        collected.push(entry);
    }

    collected.sort_by_key(|entry| entry.path());
    let mut candidates = Vec::new();
    for entry in collected {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with('.'))
        {
            continue;
        }
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                diagnostics.push(diagnostic(
                    &path,
                    SkillDiagnosticCode::UnreadableEntry,
                    SkillDiagnosticSeverity::Error,
                    format!("Cannot inspect workspace Skill entry: {error}"),
                ));
                continue;
            }
        };
        if file_type.is_symlink() {
            diagnostics.push(diagnostic(
                &path,
                SkillDiagnosticCode::SymlinkNotAllowed,
                SkillDiagnosticSeverity::Error,
                "Workspace Skill discovery does not follow symlinked Skill directories.",
            ));
        } else if file_type.is_dir() {
            candidates.push(path);
        }
    }
    (candidates, diagnostics, false)
}

fn append_duplicate_name_diagnostics(
    skills: &[SkillDescriptor],
    skills_root: &Path,
    diagnostics: &mut Vec<SkillDiagnostic>,
) {
    let mut paths_by_name = BTreeMap::<&str, Vec<&str>>::new();
    for skill in skills {
        paths_by_name
            .entry(&skill.name)
            .or_default()
            .push(&skill.relative_path);
    }
    for (name, paths) in paths_by_name {
        if paths.len() < 2 {
            continue;
        }
        diagnostics.push(diagnostic(
            skills_root,
            SkillDiagnosticCode::DuplicateName,
            SkillDiagnosticSeverity::Warning,
            format!(
                "Skill name `{name}` is declared by multiple paths: {}. Explicit selection must use Skill id.",
                paths.join(", ")
            ),
        ));
    }
}

fn catalog_with_root_diagnostic(
    path: &Path,
    code: SkillDiagnosticCode,
    message: impl Into<String>,
) -> SkillCatalog {
    finalize_catalog(
        Vec::new(),
        vec![diagnostic(
            path,
            code,
            SkillDiagnosticSeverity::Error,
            message,
        )],
        false,
    )
}

fn diagnostic(
    path: &Path,
    code: SkillDiagnosticCode,
    severity: SkillDiagnosticSeverity,
    message: impl Into<String>,
) -> SkillDiagnostic {
    SkillDiagnostic {
        code,
        severity,
        message: message.into(),
        path: diagnostic_path(path),
    }
}

fn diagnostic_path(path: &Path) -> String {
    if let Some(path) = path.to_str() {
        if !path.chars().any(char::is_control) {
            return path.to_owned();
        }
    }
    diagnostic_path_encoded(path)
}

#[cfg(unix)]
fn diagnostic_path_encoded(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;

    format!("unix-bytes:{}", percent_encode(path.as_os_str().as_bytes()))
}

#[cfg(windows)]
fn diagnostic_path_encoded(path: &Path) -> String {
    use std::fmt::Write;
    use std::os::windows::ffi::OsStrExt;

    let mut encoded = String::from("windows-wide:");
    for unit in path.as_os_str().encode_wide() {
        write!(&mut encoded, "%u{unit:04X}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(not(any(unix, windows)))]
fn diagnostic_path_encoded(path: &Path) -> String {
    format!("platform-path:{:?}", path.as_os_str())
}

fn finalize_catalog(
    skills: Vec<SkillDescriptor>,
    mut diagnostics: Vec<SkillDiagnostic>,
    truncated: bool,
) -> SkillCatalog {
    diagnostics.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.code.stable_name().cmp(right.code.stable_name()))
            .then_with(|| left.message.cmp(&right.message))
    });
    let revision_material = serde_json::to_vec(&(&skills, &diagnostics, truncated))
        .expect("Skill catalog revision material must serialize");
    SkillCatalog {
        catalog_revision: content_revision(&revision_material),
        skills,
        diagnostics,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn valid_skill(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# Instructions\n")
    }

    fn write_skill(workspace: &Path, directory: &str, contents: &[u8]) -> PathBuf {
        let skill_directory = workspace
            .join(AGENTS_DIRECTORY)
            .join(SKILLS_DIRECTORY)
            .join(directory);
        fs::create_dir_all(&skill_directory).unwrap();
        let path = skill_directory.join(SKILL_FILE_NAME);
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn discovers_the_repository_auditor_fixture() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace");

        let catalog = SkillsService::new()
            .list_workspace("fixture-workspace", &fixture)
            .unwrap();

        assert!(!catalog.truncated);
        assert!(catalog.diagnostics.is_empty());
        assert_eq!(catalog.skills.len(), 1);
        let skill = &catalog.skills[0];
        assert_eq!(
            skill.id,
            "workspace:fixture-workspace:repository-evidence-auditor"
        );
        assert_eq!(skill.name, "repository-evidence-auditor");
        assert_eq!(skill.scope, SkillScope::Workspace);
        assert_eq!(
            skill.relative_path,
            ".agents/skills/repository-evidence-auditor/SKILL.md"
        );
        assert!(!skill.revision.is_empty());
    }

    #[test]
    fn missing_skill_root_returns_a_stable_empty_catalog() {
        let workspace = tempdir().unwrap();
        let service = SkillsService::new();

        let first = service
            .list_workspace("workspace", workspace.path())
            .unwrap();
        let second = service
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert!(first.skills.is_empty());
        assert!(first.diagnostics.is_empty());
        assert_eq!(first.catalog_revision, second.catalog_revision);
    }

    #[test]
    fn isolates_invalid_skills_and_sorts_valid_skills() {
        let workspace = tempdir().unwrap();
        write_skill(
            workspace.path(),
            "zeta",
            valid_skill("zeta", "Zeta skill.").as_bytes(),
        );
        write_skill(workspace.path(), "broken", b"not frontmatter");
        write_skill(
            workspace.path(),
            "alpha",
            valid_skill("alpha", "Alpha skill.").as_bytes(),
        );

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert_eq!(
            catalog
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );
        assert_eq!(catalog.diagnostics.len(), 1);
        assert_eq!(
            catalog.diagnostics[0].code,
            SkillDiagnosticCode::MissingFrontmatter
        );
    }

    #[test]
    fn revision_tracks_the_exact_skill_bytes_while_id_stays_path_based() {
        let workspace = tempdir().unwrap();
        let first_contents = concat!(
            "---\n",
            "name: auditor\n",
            "description: Audit a repository.\n",
            "---\n",
            "# Instructions\n",
            "Version one.\n"
        );
        let skill_path = write_skill(workspace.path(), "auditor", first_contents.as_bytes());
        let service = SkillsService::new();
        let first = service
            .list_workspace("workspace", workspace.path())
            .unwrap();

        fs::write(
            skill_path,
            first_contents.replace("Version one.", "Version two."),
        )
        .unwrap();
        let second = service
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert_eq!(first.skills[0].id, second.skills[0].id);
        assert_ne!(first.skills[0].revision, second.skills[0].revision);
        assert_ne!(first.catalog_revision, second.catalog_revision);
    }

    #[test]
    fn skill_ids_are_scoped_to_the_project_identity() {
        let root = tempdir().unwrap();
        let first_workspace = root.path().join("first");
        let second_workspace = root.path().join("second");
        let contents = valid_skill("auditor", "Audit a repository.");
        write_skill(&first_workspace, "auditor", contents.as_bytes());
        write_skill(&second_workspace, "auditor", contents.as_bytes());
        let service = SkillsService::new();

        let first = service
            .list_workspace("project:one", &first_workspace)
            .unwrap();
        let second = service
            .list_workspace("project:two", &second_workspace)
            .unwrap();

        assert_eq!(first.skills[0].id, "workspace:project%3Aone:auditor");
        assert_eq!(second.skills[0].id, "workspace:project%3Atwo:auditor");
        assert_ne!(first.skills[0].id, second.skills[0].id);
        assert_eq!(first.skills[0].revision, second.skills[0].revision);
    }

    #[test]
    fn preserves_duplicate_names_without_overwriting() {
        let workspace = tempdir().unwrap();
        write_skill(
            workspace.path(),
            "first",
            valid_skill("shared-name", "First path.").as_bytes(),
        );
        write_skill(
            workspace.path(),
            "second",
            valid_skill("shared-name", "Second path.").as_bytes(),
        );

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert_eq!(catalog.skills.len(), 2);
        assert_ne!(catalog.skills[0].id, catalog.skills[1].id);
        assert!(catalog
            .diagnostics
            .iter()
            .any(|item| item.code == SkillDiagnosticCode::DuplicateName));
    }

    #[test]
    fn rejects_wrong_case_invalid_utf8_nul_and_oversized_files() {
        let workspace = tempdir().unwrap();
        let wrong_case = workspace
            .path()
            .join(AGENTS_DIRECTORY)
            .join(SKILLS_DIRECTORY)
            .join("wrong-case");
        fs::create_dir_all(&wrong_case).unwrap();
        fs::write(
            wrong_case.join("skill.md"),
            valid_skill("wrong-case", "Wrong case."),
        )
        .unwrap();
        write_skill(workspace.path(), "invalid-utf8", &[0xff, 0xfe]);
        write_skill(
            workspace.path(),
            "nul-byte",
            b"---\nname: nul-byte\ndescription: Invalid.\n---\n\0",
        );
        write_skill(
            workspace.path(),
            "oversized",
            &vec![b'x'; MAX_SKILL_FILE_BYTES + 1],
        );

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();
        let codes = catalog
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();

        assert!(codes.contains(&SkillDiagnosticCode::MissingSkillFile));
        assert!(codes.contains(&SkillDiagnosticCode::InvalidUtf8));
        assert!(codes.contains(&SkillDiagnosticCode::NulByte));
        assert!(codes.contains(&SkillDiagnosticCode::SkillFileTooLarge));
        assert!(catalog.skills.is_empty());
    }

    #[test]
    fn accepts_a_skill_file_exactly_at_the_size_limit() {
        let workspace = tempdir().unwrap();
        let mut contents = valid_skill("at-limit", "Exactly at the byte limit.").into_bytes();
        contents.resize(MAX_SKILL_FILE_BYTES, b' ');
        write_skill(workspace.path(), "at-limit", &contents);

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert_eq!(catalog.skills.len(), 1);
        assert!(catalog.diagnostics.is_empty());
    }

    #[test]
    fn defaults_missing_name_but_reports_a_warning() {
        let workspace = tempdir().unwrap();
        write_skill(
            workspace.path(),
            "fallback-name",
            b"---\ndescription: Uses its directory name.\n---\n# Instructions\n",
        );

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert_eq!(catalog.skills[0].name, "fallback-name");
        assert_eq!(
            catalog.diagnostics[0].code,
            SkillDiagnosticCode::DefaultedName
        );
        assert_eq!(
            catalog.diagnostics[0].severity,
            SkillDiagnosticSeverity::Warning
        );
    }

    #[test]
    fn excludes_metadata_only_files_that_have_no_instructions() {
        let workspace = tempdir().unwrap();
        write_skill(
            workspace.path(),
            "metadata-only",
            b"---\nname: metadata-only\ndescription: Has no body.\n---\n \t\n",
        );

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert!(catalog.skills.is_empty());
        assert_eq!(catalog.diagnostics.len(), 1);
        assert_eq!(
            catalog.diagnostics[0].code,
            SkillDiagnosticCode::MissingInstructions
        );
    }

    #[cfg(unix)]
    #[test]
    fn excludes_non_portable_skill_directory_names() {
        let workspace = tempdir().unwrap();
        write_skill(
            workspace.path(),
            "back\\slash",
            valid_skill("backslash", "Invalid directory.").as_bytes(),
        );
        write_skill(
            workspace.path(),
            "line\nbreak",
            valid_skill("control", "Invalid directory.").as_bytes(),
        );

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert!(catalog.skills.is_empty());
        assert_eq!(
            catalog
                .diagnostics
                .iter()
                .filter(|item| item.code == SkillDiagnosticCode::InvalidDirectoryName)
                .count(),
            2
        );
        let diagnostic_paths = catalog
            .diagnostics
            .iter()
            .map(|item| item.path.as_str())
            .collect::<Vec<_>>();
        assert!(diagnostic_paths
            .iter()
            .all(|path| !path.chars().any(char::is_control)));
        assert!(diagnostic_paths
            .iter()
            .any(|path| path.starts_with("unix-bytes:")));
        assert_ne!(diagnostic_paths[0], diagnostic_paths[1]);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_skill_directories_and_files() {
        use std::os::unix::fs::symlink;

        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_skill = outside.path().join(SKILL_FILE_NAME);
        fs::write(
            &outside_skill,
            valid_skill("outside", "Must not be loaded."),
        )
        .unwrap();

        let skills_root = workspace
            .path()
            .join(AGENTS_DIRECTORY)
            .join(SKILLS_DIRECTORY);
        fs::create_dir_all(&skills_root).unwrap();
        symlink(outside.path(), skills_root.join("linked-directory")).unwrap();
        let linked_file_directory = skills_root.join("linked-file");
        fs::create_dir(&linked_file_directory).unwrap();
        symlink(&outside_skill, linked_file_directory.join(SKILL_FILE_NAME)).unwrap();

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert!(catalog.skills.is_empty());
        assert_eq!(
            catalog
                .diagnostics
                .iter()
                .filter(|item| item.code == SkillDiagnosticCode::SymlinkNotAllowed)
                .count(),
            2
        );
        assert!(!catalog
            .diagnostics
            .iter()
            .any(|item| item.message.contains("Must not be loaded")));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_agents_root() {
        use std::os::unix::fs::symlink;

        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        symlink(outside.path(), workspace.path().join(AGENTS_DIRECTORY)).unwrap();

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert!(catalog.skills.is_empty());
        assert_eq!(catalog.diagnostics.len(), 1);
        assert_eq!(
            catalog.diagnostics[0].code,
            SkillDiagnosticCode::SymlinkNotAllowed
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_skills_root() {
        use std::os::unix::fs::symlink;

        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let agents_root = workspace.path().join(AGENTS_DIRECTORY);
        fs::create_dir(&agents_root).unwrap();
        symlink(outside.path(), agents_root.join(SKILLS_DIRECTORY)).unwrap();

        let catalog = SkillsService::new()
            .list_workspace("workspace", workspace.path())
            .unwrap();

        assert!(catalog.skills.is_empty());
        assert_eq!(catalog.diagnostics.len(), 1);
        assert_eq!(
            catalog.diagnostics[0].code,
            SkillDiagnosticCode::SymlinkNotAllowed
        );
    }

    #[cfg(unix)]
    #[test]
    fn losslessly_distinguishes_non_utf8_diagnostic_paths() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let first_path = PathBuf::from("/workspace").join(OsString::from_vec(vec![b'a', 0xff]));
        let second_path = PathBuf::from("/workspace").join(OsString::from_vec(vec![b'a', 0xfe]));
        let first_diagnostic = diagnostic(
            &first_path,
            SkillDiagnosticCode::UnsupportedPathEncoding,
            SkillDiagnosticSeverity::Error,
            "unsupported path",
        );
        let second_diagnostic = diagnostic(
            &second_path,
            SkillDiagnosticCode::UnsupportedPathEncoding,
            SkillDiagnosticSeverity::Error,
            "unsupported path",
        );
        let first_catalog = finalize_catalog(Vec::new(), vec![first_diagnostic.clone()], false);
        let second_catalog = finalize_catalog(Vec::new(), vec![second_diagnostic.clone()], false);

        assert!(first_diagnostic.path.starts_with("unix-bytes:"));
        assert!(second_diagnostic.path.starts_with("unix-bytes:"));
        assert_ne!(first_diagnostic.path, second_diagnostic.path);
        assert_ne!(
            first_catalog.catalog_revision,
            second_catalog.catalog_revision
        );
    }

    #[test]
    fn exact_file_lookup_consumes_a_global_entry_budget() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join(SKILL_FILE_NAME), "skill").unwrap();
        fs::write(directory.path().join("reference.md"), "reference").unwrap();
        let mut budget = ScanBudget::new(1);

        let result = find_exact_skill_file(directory.path(), &mut budget).unwrap();

        assert!(matches!(result, ExactSkillFile::ScanBudgetExceeded));
    }

    #[cfg(unix)]
    #[test]
    fn verified_read_rejects_a_file_replaced_after_validation() {
        let workspace = tempdir().unwrap();
        let skill_path = write_skill(
            workspace.path(),
            "auditor",
            valid_skill("auditor", "Original bytes.").as_bytes(),
        );
        let expected_metadata = fs::symlink_metadata(&skill_path).unwrap();
        let expected_canonical_path = skill_path.canonicalize().unwrap();
        let canonical_skill_directory = skill_path.parent().unwrap().canonicalize().unwrap();
        let canonical_workspace_root = workspace.path().canonicalize().unwrap();
        fs::rename(&skill_path, skill_path.with_extension("checked")).unwrap();
        fs::write(&skill_path, valid_skill("auditor", "Replacement bytes.")).unwrap();
        let mut byte_budget = ByteBudget::new(MAX_SKILL_CATALOG_BYTES);

        let result = read_bounded_verified(
            &skill_path,
            &expected_canonical_path,
            &expected_metadata,
            &canonical_skill_directory,
            &canonical_workspace_root,
            MAX_SKILL_FILE_BYTES,
            &mut byte_budget,
        );

        assert!(matches!(result, Err(BoundedReadError::PathChanged(_))));
    }

    #[test]
    fn catalog_byte_budget_counts_bytes_read_from_oversized_files() {
        let directory = tempdir().unwrap();
        let first_path = directory.path().join("first");
        let second_path = directory.path().join("second");
        fs::write(&first_path, b"12345").unwrap();
        fs::write(&second_path, b"12345").unwrap();
        let mut byte_budget = ByteBudget::new(6);

        let first = read_open_file_bounded(File::open(first_path).unwrap(), 3, &mut byte_budget);
        assert!(matches!(first, Err(BoundedReadError::TooLarge)));
        assert_eq!(byte_budget.remaining_bytes, 2);

        let second = read_open_file_bounded(File::open(second_path).unwrap(), 3, &mut byte_budget);
        assert_eq!(byte_budget.remaining_bytes, 0);
        assert!(matches!(
            second,
            Err(BoundedReadError::CatalogBudgetExceeded)
        ));
    }

    #[test]
    fn normalizes_windows_verbatim_drive_and_unc_paths_for_handle_comparison() {
        let drive = normalize_windows_path_units(
            r"\\?\C:\Workspace\Skills\Audit\SKILL.md"
                .encode_utf16()
                .collect(),
        );
        let regular_drive = normalize_windows_path_units(
            r"c:\workspace\skills\audit\skill.md"
                .encode_utf16()
                .collect(),
        );
        let unc = normalize_windows_path_units(
            r"\\?\UNC\Server\Share\Skills\SKILL.md"
                .encode_utf16()
                .collect(),
        );
        let regular_unc = normalize_windows_path_units(
            r"\\server\share\skills\skill.md".encode_utf16().collect(),
        );

        assert_eq!(drive, regular_drive);
        assert_eq!(unc, regular_unc);
    }
}
