//! Resolve one discovered Skill into an immutable, revision-bound snapshot.

use super::discovery::SkillsService;
use super::model::{
    ResolvedSkill, ResolvedSkillMetadata, SkillDiagnosticCode, SkillProvenance, SkillResolveError,
    SkillResolveRequest, SkillScope,
};
use super::parser::parse_skill_document;
use super::workspace::{
    load_workspace_skill, percent_encode, resolve_workspace_skills_root, skill_revision,
    validate_skill_directory_name, workspace_skill_id, ByteBudget, ScanBudget, WorkspaceRootError,
    WorkspaceSkillRoots, WorkspaceSkillsRoot, WorkspaceSourceIssue, MAX_SKILL_FILE_BYTES,
    MAX_SKILL_ROOT_ENTRIES, MAX_SKILL_SCAN_ENTRIES,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_SKILL_ID_BYTES: usize = 16 * 1024;
const MAX_SKILL_REVISION_BYTES: usize = 256;

impl SkillsService {
    /// Resolve the exact bytes selected from a prior workspace catalog.
    ///
    /// This method performs no prompt injection and never loads sibling files,
    /// references, assets, or scripts. A revision mismatch always fails closed.
    pub fn resolve_workspace_skill(
        &self,
        workspace_id: &str,
        workspace_root: &Path,
        request: &SkillResolveRequest,
    ) -> Result<ResolvedSkill, SkillResolveError> {
        let reference = validate_reference(workspace_id, request)?;
        let roots = match resolve_workspace_skills_root(workspace_root) {
            Ok(WorkspaceSkillsRoot::Missing) => return Err(not_found(request)),
            Ok(WorkspaceSkillsRoot::Ready(roots)) => roots,
            Err(WorkspaceRootError::Discovery(error)) => {
                return Err(SkillResolveError::Workspace(error));
            }
            Err(WorkspaceRootError::Invalid(issue)) => {
                return Err(map_source_issue(request, issue));
            }
        };

        let mut scan_budget = ScanBudget::new(MAX_SKILL_SCAN_ENTRIES);
        let skill_directory =
            find_selected_directory(&roots, workspace_id, &request.skill_id, &mut scan_budget)?
                .ok_or_else(|| not_found(request))?;
        let mut byte_budget = ByteBudget::new(MAX_SKILL_FILE_BYTES.saturating_add(1));
        let loaded =
            load_workspace_skill(&roots, &skill_directory, &mut scan_budget, &mut byte_budget)
                .map_err(|issue| map_source_issue(request, issue))?;

        if loaded.directory_name != reference.directory_name {
            return Err(SkillResolveError::Unavailable {
                skill_id: Some(request.skill_id.clone()),
                reason: "the selected Skill directory changed during resolution".to_string(),
            });
        }

        let actual_revision = skill_revision(&loaded.bytes);
        if actual_revision != request.expected_revision {
            return Err(SkillResolveError::Stale {
                skill_id: request.skill_id.clone(),
                expected_revision: request.expected_revision.clone(),
                actual_revision,
            });
        }

        if loaded.bytes.contains(&0) {
            return Err(invalid_skill(
                request,
                SkillDiagnosticCode::NulByte,
                "SKILL.md contains a NUL byte.",
            ));
        }
        let source_text = String::from_utf8(loaded.bytes).map_err(|_| {
            invalid_skill(
                request,
                SkillDiagnosticCode::InvalidUtf8,
                "SKILL.md must be valid UTF-8.",
            )
        })?;
        let document = parse_skill_document(&source_text, &loaded.directory_name)
            .map_err(|error| invalid_skill(request, error.diagnostic_code(), error.to_string()))?;
        let source_text: Arc<str> = Arc::from(source_text);

        Ok(ResolvedSkill::new(
            ResolvedSkillMetadata {
                id: request.skill_id.clone(),
                name: document.metadata.name,
                description: document.metadata.description,
                scope: SkillScope::Workspace,
                revision: actual_revision,
                provenance: SkillProvenance::Workspace {
                    workspace_id: workspace_id.to_owned(),
                    relative_path: loaded.relative_path,
                },
            },
            source_text,
            document.instructions_range,
        ))
    }
}

struct ValidatedReference {
    directory_name: String,
}

fn validate_reference(
    workspace_id: &str,
    request: &SkillResolveRequest,
) -> Result<ValidatedReference, SkillResolveError> {
    if request.skill_id.is_empty() {
        return Err(invalid_reference("skill_id must not be empty"));
    }
    if request.skill_id.len() > MAX_SKILL_ID_BYTES {
        return Err(invalid_reference(format!(
            "skill_id exceeds {MAX_SKILL_ID_BYTES} bytes"
        )));
    }
    if request.expected_revision.is_empty() {
        return Err(invalid_reference("expected_revision must not be empty"));
    }
    if request.expected_revision.len() > MAX_SKILL_REVISION_BYTES {
        return Err(invalid_reference(format!(
            "expected_revision exceeds {MAX_SKILL_REVISION_BYTES} bytes"
        )));
    }
    if !request
        .expected_revision
        .bytes()
        .all(|byte| byte.is_ascii_graphic())
    {
        return Err(invalid_reference(
            "expected_revision must contain only printable ASCII without whitespace",
        ));
    }

    let encoded = request
        .skill_id
        .strip_prefix("workspace:")
        .ok_or_else(|| invalid_reference("skill_id has an unsupported scope"))?;
    let (encoded_workspace_id, encoded_directory_name) = encoded
        .split_once(':')
        .ok_or_else(|| invalid_reference("skill_id is missing its directory component"))?;
    if encoded_directory_name.contains(':') {
        return Err(invalid_reference("skill_id has too many components"));
    }

    let decoded_workspace_id = decode_canonical_component(encoded_workspace_id, "workspace")?;
    let decoded_workspace_id = std::str::from_utf8(&decoded_workspace_id)
        .map_err(|_| invalid_reference("skill_id contains a non-UTF-8 workspace component"))?;
    if decoded_workspace_id != workspace_id {
        return Err(invalid_reference(
            "skill_id belongs to a different workspace",
        ));
    }

    let decoded_directory_name = decode_canonical_component(encoded_directory_name, "directory")?;
    let directory_name = String::from_utf8(decoded_directory_name)
        .map_err(|_| invalid_reference("skill_id contains a non-UTF-8 directory component"))?;
    if directory_name.is_empty() {
        return Err(invalid_reference(
            "skill_id has an empty directory component",
        ));
    }
    validate_skill_directory_name(&directory_name).map_err(invalid_reference)?;
    if workspace_skill_id(workspace_id, &directory_name) != request.skill_id {
        return Err(invalid_reference("skill_id is not canonically encoded"));
    }

    Ok(ValidatedReference { directory_name })
}

fn decode_canonical_component(encoded: &str, label: &str) -> Result<Vec<u8>, SkillResolveError> {
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            decoded.push(byte);
            index += 1;
            continue;
        }
        if byte != b'%' || index.saturating_add(2) >= bytes.len() {
            return Err(invalid_reference(format!(
                "skill_id has an invalid {label} encoding"
            )));
        }
        let high = uppercase_hex_value(bytes[index + 1]).ok_or_else(|| {
            invalid_reference(format!("skill_id has an invalid {label} percent escape"))
        })?;
        let low = uppercase_hex_value(bytes[index + 2]).ok_or_else(|| {
            invalid_reference(format!("skill_id has an invalid {label} percent escape"))
        })?;
        decoded.push((high << 4) | low);
        index += 3;
    }

    if percent_encode(&decoded) != encoded {
        return Err(invalid_reference(format!(
            "skill_id has a non-canonical {label} encoding"
        )));
    }
    Ok(decoded)
}

fn uppercase_hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

enum LocatedDirectory {
    Directory(PathBuf),
    NotDirectory,
    Symlink,
}

fn find_selected_directory(
    roots: &WorkspaceSkillRoots,
    workspace_id: &str,
    skill_id: &str,
    scan_budget: &mut ScanBudget,
) -> Result<Option<PathBuf>, SkillResolveError> {
    let entries =
        fs::read_dir(&roots.skills_root).map_err(|error| SkillResolveError::Unavailable {
            skill_id: Some(skill_id.to_owned()),
            reason: format!("cannot read workspace Skill root: {error}"),
        })?;
    let mut observed_entries = 0usize;
    let mut located = None;
    let mut unreadable_entry = None;

    for entry in entries {
        observed_entries = observed_entries.saturating_add(1);
        if !scan_budget.consume_entry() {
            return Err(invalid_skill_id(
                skill_id,
                SkillDiagnosticCode::ScanBudgetExceeded,
                format!(
                    "workspace Skill access exceeded its {MAX_SKILL_SCAN_ENTRIES}-entry scan budget"
                ),
            ));
        }
        if observed_entries > MAX_SKILL_ROOT_ENTRIES {
            return Err(invalid_skill_id(
                skill_id,
                SkillDiagnosticCode::TooManyEntries,
                format!("workspace Skill root contains more than {MAX_SKILL_ROOT_ENTRIES} entries"),
            ));
        }

        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                unreadable_entry.get_or_insert_with(|| error.to_string());
                continue;
            }
        };
        let Some(directory_name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if directory_name.starts_with('.')
            || workspace_skill_id(workspace_id, &directory_name) != skill_id
        {
            continue;
        }

        let file_type = entry
            .file_type()
            .map_err(|error| SkillResolveError::Unavailable {
                skill_id: Some(skill_id.to_owned()),
                reason: format!("cannot inspect the selected Skill directory: {error}"),
            })?;
        located = Some(if file_type.is_symlink() {
            LocatedDirectory::Symlink
        } else if file_type.is_dir() {
            LocatedDirectory::Directory(entry.path())
        } else {
            LocatedDirectory::NotDirectory
        });
    }

    match located {
        Some(LocatedDirectory::Directory(path)) => Ok(Some(path)),
        Some(LocatedDirectory::Symlink) => Err(invalid_skill_id(
            skill_id,
            SkillDiagnosticCode::SymlinkNotAllowed,
            "workspace Skill access does not follow symlinked Skill directories",
        )),
        Some(LocatedDirectory::NotDirectory) => Ok(None),
        None => match unreadable_entry {
            Some(reason) => Err(SkillResolveError::Unavailable {
                skill_id: Some(skill_id.to_owned()),
                reason: format!("cannot inspect an entry in the workspace Skill root: {reason}"),
            }),
            None => Ok(None),
        },
    }
}

fn map_source_issue(
    request: &SkillResolveRequest,
    issue: WorkspaceSourceIssue,
) -> SkillResolveError {
    match issue.code {
        SkillDiagnosticCode::MissingSkillFile => not_found(request),
        SkillDiagnosticCode::UnreadableEntry | SkillDiagnosticCode::PathChangedDuringRead => {
            SkillResolveError::Unavailable {
                skill_id: Some(request.skill_id.clone()),
                reason: issue.message,
            }
        }
        code => invalid_skill(request, code, issue.message),
    }
}

fn invalid_reference(reason: impl Into<String>) -> SkillResolveError {
    SkillResolveError::InvalidReference {
        reason: reason.into(),
    }
}

fn not_found(request: &SkillResolveRequest) -> SkillResolveError {
    SkillResolveError::NotFound {
        skill_id: request.skill_id.clone(),
    }
}

fn invalid_skill(
    request: &SkillResolveRequest,
    code: SkillDiagnosticCode,
    reason: impl Into<String>,
) -> SkillResolveError {
    invalid_skill_id(&request.skill_id, code, reason)
}

fn invalid_skill_id(
    skill_id: &str,
    code: SkillDiagnosticCode,
    reason: impl Into<String>,
) -> SkillResolveError {
    SkillResolveError::InvalidSkill {
        skill_id: skill_id.to_owned(),
        code,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::SkillDiscoveryError;
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn skill_document(name: &str, description: &str, instructions: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n{instructions}")
    }

    fn write_skill(workspace: &Path, directory: &str, contents: &[u8]) -> PathBuf {
        let skill_directory = workspace
            .join(super::super::workspace::AGENTS_DIRECTORY)
            .join(super::super::workspace::SKILLS_DIRECTORY)
            .join(directory);
        fs::create_dir_all(&skill_directory).unwrap();
        let path = skill_directory.join(super::super::workspace::SKILL_FILE_NAME);
        fs::write(&path, contents).unwrap();
        path
    }

    fn catalog_request(
        service: &SkillsService,
        workspace_id: &str,
        workspace: &Path,
        directory: &str,
    ) -> SkillResolveRequest {
        let catalog = service.list_workspace(workspace_id, workspace).unwrap();
        let descriptor = catalog
            .skills
            .iter()
            .find(|skill| skill.id == workspace_skill_id(workspace_id, directory))
            .unwrap();
        SkillResolveRequest {
            skill_id: descriptor.id.clone(),
            expected_revision: descriptor.revision.clone(),
        }
    }

    fn direct_request(workspace_id: &str, directory: &str, contents: &[u8]) -> SkillResolveRequest {
        SkillResolveRequest {
            skill_id: workspace_skill_id(workspace_id, directory),
            expected_revision: skill_revision(contents),
        }
    }

    fn workspace_provenance(resolved: &ResolvedSkill) -> (&str, &str) {
        match resolved.provenance() {
            SkillProvenance::Workspace {
                workspace_id,
                relative_path,
            } => (workspace_id, relative_path),
        }
    }

    #[test]
    fn resolves_the_fixture_into_an_exact_source_snapshot() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace");
        let service = SkillsService::new();
        let request = catalog_request(
            &service,
            "fixture-workspace",
            &workspace,
            "repository-evidence-auditor",
        );
        let descriptor = service
            .list_workspace("fixture-workspace", &workspace)
            .unwrap()
            .skills
            .into_iter()
            .next()
            .unwrap();

        let resolved = service
            .resolve_workspace_skill("fixture-workspace", &workspace, &request)
            .unwrap();
        let source = fs::read_to_string(&descriptor.path).unwrap();

        assert_eq!(resolved.id(), descriptor.id);
        assert_eq!(resolved.name(), descriptor.name);
        assert_eq!(resolved.description(), descriptor.description);
        assert_eq!(resolved.scope(), descriptor.scope);
        assert_eq!(resolved.revision(), descriptor.revision);
        let (workspace_id, relative_path) = workspace_provenance(&resolved);
        assert_eq!(workspace_id, "fixture-workspace");
        assert_eq!(relative_path, descriptor.relative_path);
        assert_eq!(resolved.source_text(), source);
        assert!(resolved.instructions().contains("SKILL_FIXTURE_V1"));
        assert_eq!(
            skill_revision(resolved.source_text().as_bytes()),
            resolved.revision()
        );
    }

    #[test]
    fn returned_snapshot_remains_frozen_after_the_source_changes() {
        let workspace = tempdir().unwrap();
        let first = skill_document("auditor", "Audit safely.", "\n# First\n");
        let path = write_skill(workspace.path(), "auditor", first.as_bytes());
        let service = SkillsService::new();
        let request = catalog_request(&service, "workspace", workspace.path(), "auditor");
        let resolved = service
            .resolve_workspace_skill("workspace", workspace.path(), &request)
            .unwrap();
        let frozen = resolved.clone();

        fs::write(
            path,
            skill_document("auditor", "Audit safely.", "\n# Replacement\n"),
        )
        .unwrap();

        assert_eq!(resolved, frozen);
        assert!(resolved.instructions().contains("# First"));
        assert!(!resolved.source_text().contains("Replacement"));
    }

    #[test]
    fn rejects_a_changed_body_as_stale_and_reports_the_current_revision() {
        let workspace = tempdir().unwrap();
        let first = skill_document("auditor", "Audit safely.", "\nVersion one.\n");
        let path = write_skill(workspace.path(), "auditor", first.as_bytes());
        let service = SkillsService::new();
        let old_request = catalog_request(&service, "workspace", workspace.path(), "auditor");
        let second = skill_document("auditor", "Audit safely.", "\nVersion two.\n");
        fs::write(path, &second).unwrap();

        let error = service
            .resolve_workspace_skill("workspace", workspace.path(), &old_request)
            .unwrap_err();
        let new_request = catalog_request(&service, "workspace", workspace.path(), "auditor");

        assert!(error.is_stale());
        assert_eq!(
            error,
            SkillResolveError::Stale {
                skill_id: old_request.skill_id,
                expected_revision: old_request.expected_revision,
                actual_revision: new_request.expected_revision.clone(),
            }
        );
        let resolved = service
            .resolve_workspace_skill("workspace", workspace.path(), &new_request)
            .unwrap();
        assert_eq!(resolved.source_text(), second);
    }

    #[test]
    fn stale_revision_takes_precedence_over_document_validation() {
        let workspace = tempdir().unwrap();
        let initial = skill_document("auditor", "Audit safely.", "\n# Run\n");
        let path = write_skill(workspace.path(), "auditor", initial.as_bytes());
        let service = SkillsService::new();
        let old_request = catalog_request(&service, "workspace", workspace.path(), "auditor");
        let invalid = [0xff, 0xfe];
        fs::write(path, invalid).unwrap();

        let stale = service
            .resolve_workspace_skill("workspace", workspace.path(), &old_request)
            .unwrap_err();
        assert_eq!(
            stale,
            SkillResolveError::Stale {
                skill_id: old_request.skill_id.clone(),
                expected_revision: old_request.expected_revision,
                actual_revision: skill_revision(&invalid),
            }
        );

        let current_request = direct_request("workspace", "auditor", &invalid);
        assert!(matches!(
            service.resolve_workspace_skill("workspace", workspace.path(), &current_request),
            Err(SkillResolveError::InvalidSkill {
                code: SkillDiagnosticCode::InvalidUtf8,
                ..
            })
        ));
    }

    #[test]
    fn preserves_bom_crlf_and_exact_instruction_boundaries() {
        let workspace = tempdir().unwrap();
        let contents = concat!(
            "\u{feff}---\r\n",
            "name: windows-auditor\r\n",
            "description: Preserve exact bytes.\r\n",
            "---\r\n",
            "\r\n",
            "# Run\r\n",
            "Keep CRLF.\r\n"
        );
        write_skill(workspace.path(), "windows-auditor", contents.as_bytes());
        let service = SkillsService::new();
        let request = catalog_request(&service, "workspace", workspace.path(), "windows-auditor");

        let resolved = service
            .resolve_workspace_skill("workspace", workspace.path(), &request)
            .unwrap();

        assert_eq!(resolved.source_text(), contents);
        assert_eq!(resolved.instructions(), "\r\n# Run\r\nKeep CRLF.\r\n");
    }

    #[test]
    fn accepts_only_the_canonical_opaque_id_for_special_directory_names() {
        let workspace = tempdir().unwrap();
        let directory = "a:b % 名";
        let contents = skill_document("special", "Special path.", "\n# Run\n");
        write_skill(workspace.path(), directory, contents.as_bytes());
        let service = SkillsService::new();
        let request = catalog_request(&service, "project:一", workspace.path(), directory);

        let resolved = service
            .resolve_workspace_skill("project:一", workspace.path(), &request)
            .unwrap();
        assert_eq!(resolved.id(), workspace_skill_id("project:一", directory));

        let non_canonical = SkillResolveRequest {
            skill_id: request.skill_id.replace("%3A", "%3a"),
            expected_revision: request.expected_revision,
        };
        assert!(matches!(
            service.resolve_workspace_skill("project:一", workspace.path(), &non_canonical),
            Err(SkillResolveError::InvalidReference { .. })
        ));
    }

    #[test]
    fn rejects_malformed_cross_workspace_and_traversal_references_before_lookup() {
        let workspace = tempdir().unwrap();
        let service = SkillsService::new();
        let invalid_ids = [
            "",
            "user:workspace:auditor",
            "workspace:workspace",
            "workspace:workspace:auditor:extra",
            "workspace:other:auditor",
            "workspace:workspace:..",
            "workspace:workspace:%2E%2E",
            "workspace:workspace:%2Ftmp",
            "workspace:workspace:%5Ctmp",
            "workspace:workspace:%00",
            "workspace:workspace:%61uditor",
            "workspace:workspace:%2ftmp",
        ];

        for skill_id in invalid_ids {
            let request = SkillResolveRequest {
                skill_id: skill_id.to_string(),
                expected_revision: "revision".to_string(),
            };
            assert!(
                matches!(
                    service.resolve_workspace_skill("workspace", workspace.path(), &request),
                    Err(SkillResolveError::InvalidReference { .. })
                ),
                "unexpected result for {skill_id}"
            );
        }
    }

    #[test]
    fn validates_request_bounds_without_scanning_the_workspace() {
        let workspace = tempdir().unwrap();
        let service = SkillsService::new();
        let requests = [
            SkillResolveRequest {
                skill_id: "x".repeat(MAX_SKILL_ID_BYTES + 1),
                expected_revision: "revision".to_string(),
            },
            SkillResolveRequest {
                skill_id: "workspace:workspace:auditor".to_string(),
                expected_revision: String::new(),
            },
            SkillResolveRequest {
                skill_id: "workspace:workspace:auditor".to_string(),
                expected_revision: "x".repeat(MAX_SKILL_REVISION_BYTES + 1),
            },
            SkillResolveRequest {
                skill_id: "workspace:workspace:auditor".to_string(),
                expected_revision: "bad revision".to_string(),
            },
        ];

        for request in requests {
            assert!(matches!(
                service.resolve_workspace_skill("workspace", workspace.path(), &request),
                Err(SkillResolveError::InvalidReference { .. })
            ));
        }
    }

    #[test]
    fn reports_deleted_skills_as_not_found_and_missing_workspaces_separately() {
        let workspace = tempdir().unwrap();
        let contents = skill_document("auditor", "Audit safely.", "\n# Run\n");
        let path = write_skill(workspace.path(), "auditor", contents.as_bytes());
        let service = SkillsService::new();
        let request = catalog_request(&service, "workspace", workspace.path(), "auditor");
        fs::remove_file(path).unwrap();

        let missing_skill = service
            .resolve_workspace_skill("workspace", workspace.path(), &request)
            .unwrap_err();
        assert!(missing_skill.is_not_found());
        assert!(!missing_skill.is_retryable());

        let absent_workspace = workspace.path().join("gone");
        let workspace_error = service
            .resolve_workspace_skill("workspace", &absent_workspace, &request)
            .unwrap_err();
        assert!(matches!(
            workspace_error,
            SkillResolveError::Workspace(SkillDiscoveryError::WorkspaceUnavailable { .. })
        ));
        assert!(workspace_error.is_retryable());
    }

    #[test]
    fn requires_an_exact_case_skill_file_name() {
        let workspace = tempdir().unwrap();
        let contents = skill_document("auditor", "Audit safely.", "\n# Run\n");
        let directory = workspace
            .path()
            .join(super::super::workspace::AGENTS_DIRECTORY)
            .join(super::super::workspace::SKILLS_DIRECTORY)
            .join("auditor");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("skill.md"), &contents).unwrap();
        let request = direct_request("workspace", "auditor", contents.as_bytes());

        let error = SkillsService::new()
            .resolve_workspace_skill("workspace", workspace.path(), &request)
            .unwrap_err();

        assert!(error.is_not_found());
    }

    #[test]
    fn rejects_empty_instructions_nul_and_oversized_sources() {
        let workspace = tempdir().unwrap();
        let empty = b"---\nname: empty\ndescription: No body.\n---\n";
        write_skill(workspace.path(), "empty", empty);
        let service = SkillsService::new();
        let empty_request = direct_request("workspace", "empty", empty);
        assert!(matches!(
            service.resolve_workspace_skill("workspace", workspace.path(), &empty_request),
            Err(SkillResolveError::InvalidSkill {
                code: SkillDiagnosticCode::MissingInstructions,
                ..
            })
        ));

        let nul = b"---\nname: nul\ndescription: NUL body.\n---\n\0";
        write_skill(workspace.path(), "nul", nul);
        let nul_request = direct_request("workspace", "nul", nul);
        assert!(matches!(
            service.resolve_workspace_skill("workspace", workspace.path(), &nul_request),
            Err(SkillResolveError::InvalidSkill {
                code: SkillDiagnosticCode::NulByte,
                ..
            })
        ));

        let oversized = vec![b'x'; MAX_SKILL_FILE_BYTES + 1];
        write_skill(workspace.path(), "oversized", &oversized);
        let oversized_request = direct_request("workspace", "oversized", &oversized);
        assert!(matches!(
            service.resolve_workspace_skill("workspace", workspace.path(), &oversized_request),
            Err(SkillResolveError::InvalidSkill {
                code: SkillDiagnosticCode::SkillFileTooLarge,
                ..
            })
        ));
    }

    #[test]
    fn accepts_a_source_exactly_at_the_file_limit() {
        let workspace = tempdir().unwrap();
        let mut contents = skill_document("at-limit", "At the limit.", "\n# Run\n").into_bytes();
        contents.resize(MAX_SKILL_FILE_BYTES, b'x');
        write_skill(workspace.path(), "at-limit", &contents);
        let service = SkillsService::new();
        let request = catalog_request(&service, "workspace", workspace.path(), "at-limit");

        let resolved = service
            .resolve_workspace_skill("workspace", workspace.path(), &request)
            .unwrap();

        assert_eq!(resolved.source_text().len(), MAX_SKILL_FILE_BYTES);
        assert_eq!(resolved.revision(), skill_revision(&contents));
    }

    #[test]
    fn unrelated_invalid_skills_do_not_block_target_resolution() {
        let workspace = tempdir().unwrap();
        let valid = skill_document("target", "Target skill.", "\n# Run\n");
        write_skill(workspace.path(), "target", valid.as_bytes());
        write_skill(
            workspace.path(),
            "oversized-sibling",
            &vec![b'x'; MAX_SKILL_FILE_BYTES + 1],
        );
        write_skill(workspace.path(), "invalid-sibling", &[0xff]);
        let service = SkillsService::new();
        let request = catalog_request(&service, "workspace", workspace.path(), "target");

        let resolved = service
            .resolve_workspace_skill("workspace", workspace.path(), &request)
            .unwrap();

        assert_eq!(resolved.name(), "target");
        assert_eq!(resolved.source_text(), valid);
    }

    #[test]
    fn refuses_to_resolve_from_an_unbounded_skill_root() {
        let workspace = tempdir().unwrap();
        let target = skill_document("target", "Target skill.", "\n# Run\n");
        write_skill(workspace.path(), "target", target.as_bytes());
        let skills_root = workspace
            .path()
            .join(super::super::workspace::AGENTS_DIRECTORY)
            .join(super::super::workspace::SKILLS_DIRECTORY);
        for index in 0..MAX_SKILL_ROOT_ENTRIES {
            fs::create_dir(skills_root.join(format!("sibling-{index:04}"))).unwrap();
        }
        let service = SkillsService::new();
        let request = direct_request("workspace", "target", target.as_bytes());

        assert!(matches!(
            service.resolve_workspace_skill("workspace", workspace.path(), &request),
            Err(SkillResolveError::InvalidSkill {
                code: SkillDiagnosticCode::TooManyEntries,
                ..
            })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_skill_directories_and_files_without_leaking_targets() {
        use std::os::unix::fs::symlink;

        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let secret = skill_document("secret", "Outside.", "\nCANARY_SECRET\n");
        let outside_file = outside.path().join("outside-skill.md");
        fs::write(&outside_file, &secret).unwrap();
        let skills_root = workspace
            .path()
            .join(super::super::workspace::AGENTS_DIRECTORY)
            .join(super::super::workspace::SKILLS_DIRECTORY);
        fs::create_dir_all(&skills_root).unwrap();
        symlink(outside.path(), skills_root.join("linked-directory")).unwrap();
        let directory_request = direct_request("workspace", "linked-directory", secret.as_bytes());
        let service = SkillsService::new();
        let directory_error = service
            .resolve_workspace_skill("workspace", workspace.path(), &directory_request)
            .unwrap_err();
        assert!(matches!(
            directory_error,
            SkillResolveError::InvalidSkill {
                code: SkillDiagnosticCode::SymlinkNotAllowed,
                ..
            }
        ));
        assert!(!directory_error.to_string().contains("CANARY_SECRET"));

        let initial = skill_document("linked-file", "Initial.", "\n# Run\n");
        let file_path = write_skill(workspace.path(), "linked-file", initial.as_bytes());
        let file_request = catalog_request(&service, "workspace", workspace.path(), "linked-file");
        fs::remove_file(&file_path).unwrap();
        symlink(&outside_file, &file_path).unwrap();
        let file_error = service
            .resolve_workspace_skill("workspace", workspace.path(), &file_request)
            .unwrap_err();
        assert!(matches!(
            file_error,
            SkillResolveError::InvalidSkill {
                code: SkillDiagnosticCode::SymlinkNotAllowed,
                ..
            }
        ));
        assert!(!file_error.to_string().contains("CANARY_SECRET"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_agents_and_skills_roots() {
        use std::os::unix::fs::symlink;

        let outside = tempdir().unwrap();
        let request = SkillResolveRequest {
            skill_id: workspace_skill_id("workspace", "auditor"),
            expected_revision: "revision".to_string(),
        };
        let service = SkillsService::new();

        let linked_agents_workspace = tempdir().unwrap();
        symlink(
            outside.path(),
            linked_agents_workspace
                .path()
                .join(super::super::workspace::AGENTS_DIRECTORY),
        )
        .unwrap();
        assert!(matches!(
            service.resolve_workspace_skill("workspace", linked_agents_workspace.path(), &request),
            Err(SkillResolveError::InvalidSkill {
                code: SkillDiagnosticCode::SymlinkNotAllowed,
                ..
            })
        ));

        let linked_skills_workspace = tempdir().unwrap();
        let agents_root = linked_skills_workspace
            .path()
            .join(super::super::workspace::AGENTS_DIRECTORY);
        fs::create_dir(&agents_root).unwrap();
        symlink(
            outside.path(),
            agents_root.join(super::super::workspace::SKILLS_DIRECTORY),
        )
        .unwrap();
        assert!(matches!(
            service.resolve_workspace_skill("workspace", linked_skills_workspace.path(), &request),
            Err(SkillResolveError::InvalidSkill {
                code: SkillDiagnosticCode::SymlinkNotAllowed,
                ..
            })
        ));
    }
}
