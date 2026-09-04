use super::super::installation_service::SkillInstallationService;
use super::super::installation_workflow::{
    SkillAcquisitionAdapterErrorCode, SkillInstallationPreparationRequest,
    SkillInstallationWorkflow, SkillPreparationId,
};
use super::super::model::SkillInstallationId;
use super::*;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::Mutex;
use tempfile::tempdir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_COMMIT: &str = "89abcdef0123456789abcdef0123456789abcdef";
const SKILL: &[u8] = b"---\nname: github-skill\ndescription: Acquired from GitHub.\n---\n\nFollow the GitHub workflow.\n";

#[derive(Default)]
struct FakeState {
    resolve_requests: Vec<GitHubResolveRequest>,
    archive_requests: Vec<GitHubArchiveRequest>,
}

struct FakeTransport {
    commit: GitHubCommit,
    archive: Vec<u8>,
    state: Mutex<FakeState>,
}

impl FakeTransport {
    fn new(archive: Vec<u8>) -> Self {
        Self {
            commit: GitHubCommit::parse(COMMIT).unwrap(),
            archive,
            state: Mutex::new(FakeState::default()),
        }
    }
}

impl GitHubAcquisitionTransport for FakeTransport {
    fn resolve_commit(
        &self,
        request: &GitHubResolveRequest,
    ) -> Result<GitHubCommit, GitHubTransportError> {
        self.state
            .lock()
            .unwrap()
            .resolve_requests
            .push(request.clone());
        Ok(self.commit.clone())
    }

    fn download_archive(
        &self,
        request: &GitHubArchiveRequest,
    ) -> Result<GitHubArchive, GitHubTransportError> {
        self.state
            .lock()
            .unwrap()
            .archive_requests
            .push(request.clone());
        GitHubArchive::from_bytes(&self.archive)
    }
}

struct SequencedTransport {
    state: Mutex<SequencedState>,
}

struct SequencedState {
    responses: VecDeque<(GitHubCommit, Vec<u8>)>,
    pending_archive: Option<Vec<u8>>,
    resolve_count: usize,
    archive_count: usize,
}

impl SequencedTransport {
    fn new(responses: Vec<(GitHubCommit, Vec<u8>)>) -> Self {
        Self {
            state: Mutex::new(SequencedState {
                responses: responses.into(),
                pending_archive: None,
                resolve_count: 0,
                archive_count: 0,
            }),
        }
    }
}

impl GitHubAcquisitionTransport for SequencedTransport {
    fn resolve_commit(
        &self,
        _request: &GitHubResolveRequest,
    ) -> Result<GitHubCommit, GitHubTransportError> {
        let mut state = self.state.lock().unwrap();
        let (commit, archive) = state
            .responses
            .pop_front()
            .ok_or(GitHubTransportError::Unavailable)?;
        state.resolve_count += 1;
        state.pending_archive = Some(archive);
        Ok(commit)
    }

    fn download_archive(
        &self,
        _request: &GitHubArchiveRequest,
    ) -> Result<GitHubArchive, GitHubTransportError> {
        let mut state = self.state.lock().unwrap();
        state.archive_count += 1;
        let bytes = state
            .pending_archive
            .take()
            .ok_or(GitHubTransportError::Unavailable)?;
        GitHubArchive::from_bytes(&bytes)
    }
}

#[test]
fn acquisition_pins_commit_extracts_only_selected_package_and_sanitizes_origin() {
    let archive = write_zip(
        &[
            ("repo-root/README.md", b"outside"),
            ("repo-root/skills/a/SKILL.md", SKILL),
            ("repo-root/skills/a/references/guide.md", b"guide"),
            ("repo-root/skills/b/SKILL.md", SKILL),
        ],
        CompressionMethod::Stored,
    );
    let transport = Arc::new(FakeTransport::new(archive));
    let acquirer = GitHubSkillAcquirer::with_transport(transport.clone());
    let location = GitHubSkillLocation::new(
        GitHubRepository::parse("openai", "skills").unwrap(),
        GitHubReference::named("release/v1").unwrap(),
        GitHubSubdirectory::parse("skills/a").unwrap(),
    );

    let acquired = acquirer.acquire(location).unwrap();

    assert_eq!(acquired.package().name(), "github-skill");
    assert_eq!(acquired.package().resource_index().len(), 1);
    assert_eq!(acquired.summary().owner(), "openai");
    assert_eq!(acquired.summary().repository(), "skills");
    assert_eq!(acquired.summary().requested_reference(), Some("release/v1"));
    assert_eq!(acquired.summary().resolved_commit().as_str(), COMMIT);
    assert_eq!(acquired.package().origin().provider(), "github");
    let origin: serde_json::Value =
        serde_json::from_str(acquired.package().origin().reference()).unwrap();
    assert_eq!(origin["owner"], "openai");
    assert_eq!(origin["repository"], "skills");
    assert_eq!(origin["resolvedCommit"], COMMIT);
    assert_eq!(origin["subdirectory"], "skills/a");
    let state = transport.state.lock().unwrap();
    assert_eq!(state.resolve_requests.len(), 1);
    assert_eq!(state.archive_requests.len(), 1);
    assert_eq!(state.archive_requests[0].commit().as_str(), COMMIT);
}

#[test]
fn tracking_refresh_reacquires_new_bytes_and_projects_typed_source_metadata() {
    let first_archive = write_zip(
        &[("repo-root/skills/a/SKILL.md", SKILL)],
        CompressionMethod::Stored,
    );
    let second_skill = b"---\nname: github-skill\ndescription: Acquired from GitHub.\n---\n\nUPDATED THROUGH TRACKING REF.\n";
    let second_archive = write_zip(
        &[("repo-root/skills/a/SKILL.md", second_skill)],
        CompressionMethod::Stored,
    );
    let transport = Arc::new(SequencedTransport::new(vec![
        (GitHubCommit::parse(COMMIT).unwrap(), first_archive),
        (GitHubCommit::parse(OTHER_COMMIT).unwrap(), second_archive),
    ]));
    let adapter = GitHubWorkflowAcquisitionAdapter::new(Arc::new(
        GitHubSkillAcquirer::with_transport(transport.clone()),
    ));
    let source = GitHubWorkflowAcquisitionAdapter::source_for(&GitHubSkillLocation::new(
        GitHubRepository::parse("openai", "skills").unwrap(),
        GitHubReference::named("main").unwrap(),
        GitHubSubdirectory::parse("skills/a").unwrap(),
    ))
    .unwrap();

    let first = adapter.acquire(&source).unwrap();
    let refresh = first.provenance().refresh().unwrap().clone();
    let second = adapter.reacquire(refresh.adapter_view()).unwrap();

    assert_ne!(first.package().revision(), second.package().revision());
    assert!(second
        .package()
        .instructions()
        .contains("UPDATED THROUGH TRACKING REF"));
    assert!(matches!(
        adapter.installed_source_presentation(second.provenance().adapter_view(), true),
        Some(InstalledSkillSourcePresentation::GitHub {
            tracking_reference: InstalledGitHubTrackingReference::Named(reference),
            resolved_commit,
            refreshable: true,
            ..
        }) if reference == "main" && resolved_commit == OTHER_COMMIT
    ));
    let state = transport.state.lock().unwrap();
    assert_eq!(state.resolve_count, 2);
    assert_eq!(state.archive_count, 2);
}

#[test]
fn immutable_commit_provenance_has_no_refresh_capability() {
    let transport = Arc::new(FakeTransport::new(write_zip(
        &[("repo-root/SKILL.md", SKILL)],
        CompressionMethod::Stored,
    )));
    let adapter = GitHubWorkflowAcquisitionAdapter::new(Arc::new(
        GitHubSkillAcquirer::with_transport(transport),
    ));
    let source = GitHubWorkflowAcquisitionAdapter::source_for(&GitHubSkillLocation::new(
        GitHubRepository::parse("openai", "skills").unwrap(),
        GitHubReference::commit(COMMIT).unwrap(),
        GitHubSubdirectory::root(),
    ))
    .unwrap();

    let acquisition = adapter.acquire(&source).unwrap();

    assert!(acquisition.provenance().refresh().is_none());
    assert!(matches!(
        adapter.installed_source_presentation(acquisition.provenance().adapter_view(), false),
        Some(InstalledSkillSourcePresentation::GitHub {
            tracking_reference: InstalledGitHubTrackingReference::Commit,
            refreshable: false,
            ..
        })
    ));
}

#[test]
fn authority_projection_rejects_unknown_payload_fields() {
    let authority = SkillInstallationAuthority::new(
        GITHUB_SKILL_ORIGIN_PROVIDER,
        GITHUB_PROVENANCE_SCHEMA_VERSION,
        serde_json::json!({
            "owner": "openai",
            "repository": "skills",
            "resolvedCommit": COMMIT,
            "unexpected": "must fail closed"
        })
        .to_string(),
    )
    .unwrap();
    let provenance = SkillInstallationProvenance::new(authority, None);

    assert!(github_source_presentation(provenance.adapter_view(), false).is_none());
}

#[test]
fn immutable_commit_requests_skip_ref_resolution_and_download_the_exact_sha() {
    let transport = Arc::new(FakeTransport::new(write_zip(
        &[("repo-root/SKILL.md", SKILL)],
        CompressionMethod::Stored,
    )));
    let acquirer = GitHubSkillAcquirer::with_transport(transport.clone());
    let location = GitHubSkillLocation::new(
        GitHubRepository::parse("openai", "skills").unwrap(),
        GitHubReference::commit(OTHER_COMMIT).unwrap(),
        GitHubSubdirectory::root(),
    );

    let acquired = acquirer.acquire(location).unwrap();

    assert_eq!(acquired.summary().resolved_commit().as_str(), OTHER_COMMIT);
    let state = transport.state.lock().unwrap();
    assert!(state.resolve_requests.is_empty());
    assert_eq!(state.archive_requests.len(), 1);
    assert_eq!(state.archive_requests[0].commit().as_str(), OTHER_COMMIT);
}

#[test]
fn workflow_adapter_owns_strict_protocol_wire_and_inspects_a_fake_transport_package() {
    let archive = write_zip(
        &[("repo-root/skills/a/SKILL.md", SKILL)],
        CompressionMethod::Stored,
    );
    let transport = Arc::new(FakeTransport::new(archive));
    let acquirer = Arc::new(GitHubSkillAcquirer::with_transport(transport));
    let adapter = Arc::new(GitHubWorkflowAcquisitionAdapter::new(acquirer));
    let location = GitHubSkillLocation::new(
        GitHubRepository::parse("openai", "skills").unwrap(),
        GitHubReference::named("main").unwrap(),
        GitHubSubdirectory::parse("skills/a").unwrap(),
    );
    let source = GitHubWorkflowAcquisitionAdapter::source_for(&location).unwrap();
    let wire: serde_json::Value =
        serde_json::from_slice(source.adapter_request().unwrap()).unwrap();
    assert_eq!(wire["kind"], "githubRepository");
    assert_eq!(wire["owner"], "openai");
    assert_eq!(wire["repository"], "skills");
    assert_eq!(wire["reference"]["kind"], "named");
    assert_eq!(wire["reference"]["value"], "main");
    assert_eq!(wire["subdirectory"], "skills/a");

    let fixture = tempdir().unwrap();
    let installation_service = SkillInstallationService::new(fixture.path().join("store")).unwrap();
    let mut workflow = SkillInstallationWorkflow::new(installation_service);
    workflow.register_adapter(adapter).unwrap();
    let request = SkillInstallationPreparationRequest::install(
        SkillPreparationId::new(),
        SkillInstallationId::parse("01234567-89ab-4def-8123-456789abcdef").unwrap(),
        source,
    );

    let preview = workflow.inspect(&request).unwrap();
    assert_eq!(preview.package().name(), "github-skill");
    assert_eq!(preview.package().format_version(), 1);
}

#[test]
fn workflow_adapter_rejects_unknown_wire_fields_before_transport() {
    let transport = Arc::new(FakeTransport::new(Vec::new()));
    let adapter = GitHubWorkflowAcquisitionAdapter::new(Arc::new(
        GitHubSkillAcquirer::with_transport(transport.clone()),
    ));
    let source = SkillAcquisitionSource::adapter(
        github_provider(),
        br#"{
                "kind":"githubRepository",
                "owner":"openai",
                "repository":"skills",
                "unexpected":"must-fail"
            }"#
        .to_vec(),
    )
    .unwrap();

    let error = adapter.acquire(&source).unwrap_err();
    assert_eq!(
        error.code(),
        SkillAcquisitionAdapterErrorCode::InvalidRequest
    );
    assert!(transport.state.lock().unwrap().resolve_requests.is_empty());
}

#[test]
fn fixed_endpoint_builders_percent_encode_refs_and_pin_codeload_to_commit() {
    let repository = GitHubRepository::parse("owner", "repo").unwrap();
    let api = github_api_commit_url(&repository, "feature/a").unwrap();
    assert_eq!(
        api.as_str(),
        "https://api.github.com/repos/owner/repo/commits/feature%2Fa"
    );
    let commit = GitHubCommit::parse(COMMIT).unwrap();
    let codeload = github_codeload_url(&repository, &commit).unwrap();
    assert_eq!(
        codeload.as_str(),
        format!("https://codeload.github.com/owner/repo/zip/{COMMIT}")
    );
}

#[test]
fn ls_remote_parser_supports_default_branch_branch_and_annotated_tag() {
    let default = parse_ls_remote_output(
        &GitHubReference::DefaultBranch,
        format!("ref: refs/heads/main\tHEAD\n{COMMIT}\tHEAD\n").as_bytes(),
    )
    .unwrap();
    assert_eq!(default.as_str(), COMMIT);

    let branch = parse_ls_remote_output(
        &GitHubReference::named("feature/a").unwrap(),
        format!("{COMMIT}\trefs/heads/feature/a\n").as_bytes(),
    )
    .unwrap();
    assert_eq!(branch.as_str(), COMMIT);

    let tag = parse_ls_remote_output(
        &GitHubReference::named("v1").unwrap(),
        format!("{OTHER_COMMIT}\trefs/tags/v1\n{COMMIT}\trefs/tags/v1^{{}}\n").as_bytes(),
    )
    .unwrap();
    assert_eq!(tag.as_str(), COMMIT);
}

#[test]
fn archive_spool_detects_interrupted_and_length_mismatched_downloads() {
    struct InterruptedReader {
        delivered: bool,
    }

    impl Read for InterruptedReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.delivered {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "fixture interruption",
                ));
            }
            self.delivered = true;
            buffer[..4].copy_from_slice(b"data");
            Ok(4)
        }
    }

    assert_eq!(
        spool_archive_reader(&mut InterruptedReader { delivered: false }, None).unwrap_err(),
        GitHubTransportError::NetworkUnavailable
    );
    assert_eq!(
        spool_archive_reader(&mut Cursor::new(b"short"), Some(99)).unwrap_err(),
        GitHubTransportError::InvalidResponse
    );
    let archive = spool_archive_reader(&mut Cursor::new(b"complete"), Some(8)).unwrap();
    assert_eq!(archive.len(), 8);
    assert!(archive.verify_integrity().is_ok());
}

#[test]
fn github_rate_limit_statuses_preserve_bounded_retry_hints() {
    let mut primary_headers = HeaderMap::new();
    primary_headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
    let primary = map_status_and_headers(StatusCode::FORBIDDEN, &primary_headers);
    assert!(matches!(
        primary,
        GitHubTransportError::RateLimited {
            retry_after: Some(duration),
            secondary: false,
        } if duration == GITHUB_RATE_LIMIT_FALLBACK
    ));

    let mut secondary_headers = HeaderMap::new();
    secondary_headers.insert(RETRY_AFTER, "7".parse().unwrap());
    let secondary = map_status_and_headers(StatusCode::TOO_MANY_REQUESTS, &secondary_headers);
    assert!(!retryable_transport_error(&secondary));
    assert!(matches!(
        secondary,
        GitHubTransportError::RateLimited {
            retry_after: Some(duration),
            secondary: true,
        } if duration == Duration::from_secs(7)
    ));

    assert_eq!(
        map_status_and_headers(StatusCode::FORBIDDEN, &HeaderMap::new()),
        GitHubTransportError::Rejected
    );
    assert!(retryable_transport_error(&GitHubTransportError::Timeout));
    assert!(!retryable_transport_error(
        &GitHubTransportError::InvalidResponse
    ));
}

#[test]
fn transport_requests_are_constructed_from_validated_coordinates() {
    let repository = GitHubRepository::parse("owner", "repo").unwrap();
    let reference = GitHubReference::named("release/v1").unwrap();
    let resolve = GitHubResolveRequest::new(repository.clone(), reference.clone());
    assert_eq!(resolve.repository(), &repository);
    assert_eq!(resolve.reference(), &reference);

    let commit = GitHubCommit::parse(COMMIT).unwrap();
    let archive = GitHubArchiveRequest::new(repository.clone(), commit.clone());
    assert_eq!(archive.repository(), &repository);
    assert_eq!(archive.commit(), &commit);
}

#[test]
fn structured_coordinates_reject_urls_partial_commits_and_unsafe_subdirectories() {
    assert!(GitHubRepository::parse("https://github.com/openai", "skills").is_err());
    assert!(GitHubRepository::parse("openai", "skills.git/other").is_err());
    assert!(GitHubReference::commit("01234567").is_err());
    assert!(GitHubReference::named("refs/../main").is_err());
    assert!(GitHubSubdirectory::parse("../skills/a").is_err());
    assert!(GitHubSubdirectory::parse("skills\\a").is_err());
    assert!(GitHubSubdirectory::parse("/skills/a").is_err());
}

#[test]
fn archive_security_corpus_rejects_absolute_parent_backslash_and_multiple_roots() {
    for unsafe_name in [
        "/repo-root/SKILL.md",
        "repo-root/../SKILL.md",
        "repo-root\\SKILL.md",
    ] {
        let archive = write_zip(&[(unsafe_name, SKILL)], CompressionMethod::Stored);
        assert_eq!(
            extract_selected_skill(&archive, &GitHubSubdirectory::root())
                .unwrap_err()
                .code(),
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
            "unsafe name: {unsafe_name}"
        );
    }

    let multiple_roots = write_zip(
        &[("root-a/SKILL.md", SKILL), ("root-b/README.md", b"other")],
        CompressionMethod::Stored,
    );
    assert_eq!(
        extract_selected_skill(&multiple_roots, &GitHubSubdirectory::root())
            .unwrap_err()
            .code(),
        GitHubAcquisitionErrorCode::UnsafeArchiveEntry
    );
}

#[test]
fn archive_security_corpus_rejects_case_collisions_symlinks_and_special_modes() {
    let collision = write_zip(
        &[("root/SKILL.md", SKILL), ("root/skill.md", b"collision")],
        CompressionMethod::Stored,
    );
    assert_eq!(
        extract_selected_skill(&collision, &GitHubSubdirectory::root())
            .unwrap_err()
            .code(),
        GitHubAcquisitionErrorCode::UnsafeArchiveEntry
    );

    let symlink = write_symlink_zip();
    assert_eq!(
        extract_selected_skill(&symlink, &GitHubSubdirectory::root())
            .unwrap_err()
            .code(),
        GitHubAcquisitionErrorCode::UnsafeArchiveEntry
    );

    let mut special = write_zip(
        &[
            ("root/SKILL.md", SKILL),
            ("root/assets/pipe", b"not-a-regular-file"),
        ],
        CompressionMethod::Stored,
    );
    set_central_unix_mode(&mut special, "root/assets/pipe", 0o010644);
    assert_eq!(
        extract_selected_skill(&special, &GitHubSubdirectory::root())
            .unwrap_err()
            .code(),
        GitHubAcquisitionErrorCode::UnsafeArchiveEntry
    );
}

#[test]
fn unrelated_repository_symlink_does_not_block_a_selected_skill() {
    let archive = write_zip_with_symlink(
        &[("root/skills/a/SKILL.md", SKILL)],
        "root/CLAUDE.md",
        "AGENTS.md",
    );

    let files =
        extract_selected_skill(&archive, &GitHubSubdirectory::parse("skills/a").unwrap()).unwrap();

    assert_eq!(files, vec![(SKILL_FILE_NAME.to_string(), SKILL.to_vec())]);
}

#[test]
fn symlinks_at_or_below_the_selected_boundary_are_rejected() {
    for link in ["root/skills", "root/skills/a", "root/skills/a/assets/link"] {
        let archive =
            write_zip_with_symlink(&[("root/skills/a/SKILL.md", SKILL)], link, "../../outside");

        assert_eq!(
            extract_selected_skill(&archive, &GitHubSubdirectory::parse("skills/a").unwrap(),)
                .unwrap_err()
                .code(),
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
            "selected boundary symlink: {link}",
        );
    }
}

#[test]
fn unrelated_collisions_and_declared_sizes_do_not_affect_selected_skill() {
    let mut archive = write_zip(
        &[
            ("root/skills/a/SKILL.md", SKILL),
            ("root/unrelated/Guide.md", b"first"),
            ("root/unrelated/guide.md", b"second"),
            ("root/unrelated/large.bin", b"small"),
        ],
        CompressionMethod::Stored,
    );
    set_central_uncompressed_size(
        &mut archive,
        "root/unrelated/large.bin",
        u32::try_from(MAX_GITHUB_ARCHIVE_ENTRY_BYTES + 1).unwrap(),
    );

    let files =
        extract_selected_skill(&archive, &GitHubSubdirectory::parse("skills/a").unwrap()).unwrap();

    assert_eq!(files, vec![(SKILL_FILE_NAME.to_string(), SKILL.to_vec())]);
}

#[test]
fn selected_collisions_and_declared_sizes_remain_rejected() {
    let collision = write_zip(
        &[
            ("root/skills/a/SKILL.md", SKILL),
            ("root/skills/a/assets/Guide.md", b"first"),
            ("root/skills/a/assets/guide.md", b"second"),
        ],
        CompressionMethod::Stored,
    );
    assert_eq!(
        extract_selected_skill(&collision, &GitHubSubdirectory::parse("skills/a").unwrap(),)
            .unwrap_err()
            .code(),
        GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
    );

    let mut oversized = write_zip(
        &[
            ("root/skills/a/SKILL.md", SKILL),
            ("root/skills/a/assets/large.bin", b"small"),
        ],
        CompressionMethod::Stored,
    );
    set_central_uncompressed_size(
        &mut oversized,
        "root/skills/a/assets/large.bin",
        u32::try_from(MAX_GITHUB_ARCHIVE_ENTRY_BYTES + 1).unwrap(),
    );
    assert_eq!(
        extract_selected_skill(&oversized, &GitHubSubdirectory::parse("skills/a").unwrap(),)
            .unwrap_err()
            .code(),
        GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
    );
}

#[test]
fn compression_expansion_is_scoped_to_selected_entries() {
    let highly_compressible = vec![0_u8; 4 * 1024 * 1024];
    let archive = write_zip(
        &[
            ("root/skills/a/SKILL.md", SKILL),
            ("root/unrelated/bomb.bin", &highly_compressible),
        ],
        CompressionMethod::Deflated,
    );

    assert!(
        extract_selected_skill(&archive, &GitHubSubdirectory::parse("skills/a").unwrap(),).is_ok()
    );
}

#[test]
fn archive_security_corpus_rejects_invalid_utf8_and_zip_bombs_before_extraction() {
    let mut invalid_utf8 = write_zip(
        &[("root/SKILL.md", SKILL), ("root/assets/bad.bin", b"bad")],
        CompressionMethod::Stored,
    );
    replace_all(&mut invalid_utf8, b"bad.bin", b"\xffad.bin");
    assert_eq!(
        extract_selected_skill(&invalid_utf8, &GitHubSubdirectory::root())
            .unwrap_err()
            .code(),
        GitHubAcquisitionErrorCode::UnsafeArchiveEntry
    );

    let bomb = vec![0_u8; 4 * 1024 * 1024];
    let archive = write_zip(
        &[("root/SKILL.md", SKILL), ("root/assets/bomb.bin", &bomb)],
        CompressionMethod::Deflated,
    );
    assert_eq!(
        extract_selected_skill(&archive, &GitHubSubdirectory::root())
            .unwrap_err()
            .code(),
        GitHubAcquisitionErrorCode::ArchiveBudgetExceeded
    );
}

#[test]
fn unrelated_nonportable_names_do_not_block_a_selected_skill() {
    let mut archive = write_zip(
        &[
            ("root/skills/a/SKILL.md", SKILL),
            ("root/unrelated/unique-name.bin", b"outside"),
            ("root/unrelated/legacy:name.txt", b"outside"),
        ],
        CompressionMethod::Stored,
    );
    replace_all(&mut archive, b"unique-name.bin", b"unique-nam\xff.bin");

    let files =
        extract_selected_skill(&archive, &GitHubSubdirectory::parse("skills/a").unwrap()).unwrap();

    assert_eq!(files, vec![(SKILL_FILE_NAME.to_string(), SKILL.to_vec())]);
}

#[test]
fn selected_nonportable_names_remain_rejected() {
    let archive = write_zip(
        &[
            ("root/skills/a/SKILL.md", SKILL),
            ("root/skills/a/assets/legacy:name.txt", b"selected"),
        ],
        CompressionMethod::Stored,
    );

    assert_eq!(
        extract_selected_skill(&archive, &GitHubSubdirectory::parse("skills/a").unwrap())
            .unwrap_err()
            .code(),
        GitHubAcquisitionErrorCode::UnsafeArchiveEntry
    );
}

#[test]
fn selected_directory_preserves_safe_generic_sibling_files() {
    let archive = write_zip(
        &[
            ("root/SKILL.md", SKILL),
            ("root/README.md", b"not a package resource"),
        ],
        CompressionMethod::Stored,
    );
    assert_eq!(
        extract_selected_skill(&archive, &GitHubSubdirectory::root()).unwrap(),
        vec![
            (SKILL_FILE_NAME.to_string(), SKILL.to_vec()),
            ("README.md".to_string(), b"not a package resource".to_vec()),
        ]
    );
}

fn write_zip(entries: &[(&str, &[u8])], compression: CompressionMethod) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default()
        .compression_method(compression)
        .unix_permissions(0o644);
    for (name, bytes) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn write_zip_with_symlink(entries: &[(&str, &[u8])], symlink_name: &str, target: &str) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().unix_permissions(0o644);
    for (name, bytes) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.add_symlink(symlink_name, target, options).unwrap();
    writer.finish().unwrap().into_inner()
}

fn write_symlink_zip() -> Vec<u8> {
    write_zip_with_symlink(
        &[("root/SKILL.md", SKILL)],
        "root/assets/link",
        "../../outside",
    )
}

fn replace_all(bytes: &mut [u8], needle: &[u8], replacement: &[u8]) {
    assert_eq!(needle.len(), replacement.len());
    let mut offset = 0;
    let mut replacements = 0;
    while let Some(relative) = bytes[offset..]
        .windows(needle.len())
        .position(|window| window == needle)
    {
        let start = offset + relative;
        bytes[start..start + needle.len()].copy_from_slice(replacement);
        offset = start + needle.len();
        replacements += 1;
    }
    assert!(
        replacements >= 2,
        "local and central names must be replaced"
    );
}

fn set_central_unix_mode(bytes: &mut [u8], entry_name: &str, mode: u32) {
    const CENTRAL_SIGNATURE: &[u8] = b"PK\x01\x02";
    let mut offset = 0;
    while let Some(relative) = bytes[offset..]
        .windows(CENTRAL_SIGNATURE.len())
        .position(|window| window == CENTRAL_SIGNATURE)
    {
        let start = offset + relative;
        let name_length = u16::from_le_bytes([bytes[start + 28], bytes[start + 29]]) as usize;
        let name_start = start + 46;
        let name_end = name_start + name_length;
        if bytes.get(name_start..name_end) == Some(entry_name.as_bytes()) {
            bytes[start + 5] = 3;
            bytes[start + 38..start + 42].copy_from_slice(&(mode << 16).to_le_bytes());
            return;
        }
        offset = name_end;
    }
    panic!("central entry not found");
}

fn set_central_uncompressed_size(bytes: &mut [u8], entry_name: &str, size: u32) {
    const CENTRAL_SIGNATURE: &[u8] = b"PK\x01\x02";
    let mut offset = 0;
    while let Some(relative) = bytes[offset..]
        .windows(CENTRAL_SIGNATURE.len())
        .position(|window| window == CENTRAL_SIGNATURE)
    {
        let start = offset + relative;
        let name_length = u16::from_le_bytes([bytes[start + 28], bytes[start + 29]]) as usize;
        let name_start = start + 46;
        let name_end = name_start + name_length;
        if bytes.get(name_start..name_end) == Some(entry_name.as_bytes()) {
            bytes[start + 24..start + 28].copy_from_slice(&size.to_le_bytes());
            return;
        }
        offset = name_end;
    }
    panic!("central entry not found");
}
