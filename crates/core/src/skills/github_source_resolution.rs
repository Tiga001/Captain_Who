//! Public GitHub URL resolver for human-friendly Skill installation.
//!
//! URLs are locators, never acquisition authority. Every accepted ref is resolved to a full
//! commit, every discovered candidate is prepared through the same package validator used by the
//! installation workflow, and the result exposes only immutable commit coordinates.

use super::github_acquisition::{
    extract_selected_skill, validate_archive_entry_path, GitHubAcquisitionError,
    GitHubAcquisitionErrorCode, GitHubAcquisitionTransport, GitHubArchiveRequest, GitHubCommit,
    GitHubReference, GitHubRepository, GitHubResolveRequest, GitHubSubdirectory,
    GitHubTransportError, MAX_GITHUB_ARCHIVE_ENTRIES, MAX_GITHUB_ZIP_BYTES,
};
use super::origin::SkillPackageOrigin;
use super::prepared::PreparedSkillPackage;
use super::source_resolution::{
    ResolvedSkillPackagePreview, ResolvedSkillSource, SkillInstallationSourceLocator,
    SkillInstallationSourceResolver, SkillSourceResolution, SkillSourceResolutionCandidate,
    SkillSourceResolutionError, SkillSourceResolutionErrorCode, SkillSourceResolutionRecovery,
    SkillSourceResolverId,
};
use super::workspace::SKILL_FILE_NAME;
use reqwest::Url;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::sync::Arc;
use zip::ZipArchive;

const GITHUB_RESOLVER_ID: &str = "github";
const GITHUB_HOSTS: &[&str] = &["github.com", "raw.githubusercontent.com", "www.github.com"];
const MAX_GITHUB_URL_COMPONENTS: usize = 64;
const MAX_GITHUB_REF_INTERPRETATIONS: usize = 8;
const MAX_DISCOVERED_SKILLS: usize = 64;
const MAX_RESOLUTION_ARCHIVES: usize = 2;
const MAX_RESOLUTION_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
const MAX_RESOLUTION_SKILL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RESOLUTION_SKILL_FILES: u64 = 4_096;
const MAX_RESOLUTION_CANDIDATE_ATTEMPTS: u64 = 128;

pub struct GitHubInstallationSourceResolver {
    transport: Arc<dyn GitHubAcquisitionTransport>,
}

impl std::fmt::Debug for GitHubInstallationSourceResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GitHubInstallationSourceResolver")
            .finish_non_exhaustive()
    }
}

impl GitHubInstallationSourceResolver {
    pub fn new(transport: Arc<dyn GitHubAcquisitionTransport>) -> Self {
        Self { transport }
    }

    fn resolve_github_url(
        &self,
        locator: &SkillInstallationSourceLocator,
    ) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
        let parsed = ParsedGitHubUrl::parse(locator.as_url())?;

        match parsed.target.clone() {
            GitHubUrlTarget::Repository => {
                let default_commit = self
                    .transport
                    .resolve_commit(&GitHubResolveRequest::new(
                        parsed.repository.clone(),
                        GitHubReference::DefaultBranch,
                    ))
                    .map_err(map_repository_transport_error)?;
                let archive = self.download_archive(&parsed.repository, &default_commit)?;
                let mut preparation_budget = ResolutionPreparationBudget::default();
                let candidates = prepare_candidates(
                    &archive,
                    &parsed.repository,
                    &default_commit,
                    &GitHubSubdirectory::root(),
                    CandidateSelection::Discover,
                    &mut preparation_budget,
                )?;
                resolution_from_candidates(
                    parsed.canonical_url(None, None)?,
                    default_commit,
                    candidates,
                )
            }
            GitHubUrlTarget::Tree { tail } | GitHubUrlTarget::SkillFile { tail } => {
                self.resolve_ref_and_scope(parsed, &tail)
            }
        }
    }

    fn resolve_ref_and_scope(
        &self,
        parsed: ParsedGitHubUrl,
        tail: &[String],
    ) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
        let direct_skill = matches!(parsed.target, GitHubUrlTarget::SkillFile { .. });
        let interpretations = reference_interpretations(tail, direct_skill)?;
        let reference_requests = interpretations
            .into_iter()
            .filter_map(|interpretation| {
                let reference = if is_full_sha(&interpretation.reference) {
                    GitHubReference::commit(interpretation.reference.clone())
                } else {
                    GitHubReference::named(interpretation.reference.clone())
                };
                reference.ok().map(|reference| (interpretation, reference))
            })
            .collect::<Vec<_>>();
        if reference_requests.is_empty() {
            return Err(invalid_url_shape(
                "The GitHub URL does not contain a valid branch, tag, or commit reference.",
            ));
        }
        // Slash-bearing ref names require several interpretations. Resolve those small API
        // requests concurrently so one maliciously ambiguous URL cannot serialize a full timeout
        // per interpretation on the shared acquisition lane.
        let reference_results = std::thread::scope(|scope| {
            let handles = reference_requests
                .into_iter()
                .map(|(interpretation, reference)| {
                    let transport = Arc::clone(&self.transport);
                    let repository = parsed.repository.clone();
                    scope.spawn(move || {
                        let result = transport
                            .resolve_commit(&GitHubResolveRequest::new(repository, reference));
                        (interpretation, result)
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().map_err(|_| unavailable()))
                .collect::<Result<Vec<_>, SkillSourceResolutionError>>()
        })?;
        let mut resolved = Vec::new();
        let mut reference_was_found = false;
        let mut archives = BTreeMap::<String, Vec<u8>>::new();
        let mut downloaded_archive_bytes = 0_usize;
        let mut first_candidate_error = None;
        let mut preparation_budget = ResolutionPreparationBudget::default();

        for (interpretation, reference_result) in reference_results {
            let commit = match reference_result {
                Ok(commit) => commit,
                // GitHub returns both 404 and 422 for non-existent/unsuitable pseudo refs. A
                // rejected longer split must not poison a valid shorter branch interpretation.
                Err(GitHubTransportError::NotFound | GitHubTransportError::Rejected) => continue,
                Err(error) => return Err(map_reference_transport_error(error)),
            };
            reference_was_found = true;
            if is_full_sha(&interpretation.reference)
                && commit.as_str() != interpretation.reference.to_ascii_lowercase()
            {
                return Err(unavailable());
            }
            if !archives.contains_key(commit.as_str()) {
                if archives.len() >= MAX_RESOLUTION_ARCHIVES {
                    return Err(repository_too_large());
                }
                let archive = self.download_archive(&parsed.repository, &commit)?;
                downloaded_archive_bytes = downloaded_archive_bytes
                    .checked_add(archive.len())
                    .filter(|total| *total <= MAX_RESOLUTION_ARCHIVE_BYTES)
                    .ok_or_else(repository_too_large)?;
                archives.insert(commit.as_str().to_string(), archive);
            }
            let archive = archives
                .get(commit.as_str())
                .expect("resolved commit archive must be cached");
            let selection = if direct_skill {
                CandidateSelection::Exact
            } else {
                CandidateSelection::Discover
            };
            match prepare_candidates(
                archive,
                &parsed.repository,
                &commit,
                &interpretation.scope,
                selection,
                &mut preparation_budget,
            ) {
                Ok(candidates) => resolved.push(ResolvedInterpretation {
                    candidates,
                    commit,
                    reference: interpretation.reference,
                    scope: interpretation.scope,
                }),
                Err(error)
                    if matches!(
                        error.code(),
                        SkillSourceResolutionErrorCode::PathNotFound
                            | SkillSourceResolutionErrorCode::NoSkillsFound
                            | SkillSourceResolutionErrorCode::InvalidPackage
                            | SkillSourceResolutionErrorCode::UnsafePackage
                    ) =>
                {
                    first_candidate_error.get_or_insert(error);
                }
                Err(error) => return Err(error),
            }
        }

        if resolved.is_empty() {
            if let Some(error) = first_candidate_error {
                return Err(error);
            }
            return Err(SkillSourceResolutionError::resolve(
                if reference_was_found {
                    SkillSourceResolutionErrorCode::PathNotFound
                } else {
                    SkillSourceResolutionErrorCode::ReferenceNotFound
                },
                SkillSourceResolutionRecovery::FixLocator,
                if reference_was_found {
                    "The GitHub URL path was not found at the resolved reference."
                } else {
                    "The GitHub URL branch, tag, or commit was not found."
                },
            ));
        }

        // Different textual refs that freeze to exactly the same content are not a security
        // ambiguity. Prefer the longest valid ref because GitHub branch names may contain '/'.
        resolved.sort_by(|left, right| {
            right
                .reference
                .len()
                .cmp(&left.reference.len())
                .then_with(|| left.reference.cmp(&right.reference))
        });
        let first_fingerprint = resolved[0].fingerprint();
        if resolved
            .iter()
            .skip(1)
            .any(|candidate| candidate.fingerprint() != first_fingerprint)
        {
            return Err(SkillSourceResolutionError::resolve(
                SkillSourceResolutionErrorCode::AmbiguousReference,
                SkillSourceResolutionRecovery::FixLocator,
                "The GitHub URL can resolve to more than one branch/path combination. Use a commit permalink.",
            ));
        }

        let chosen = resolved.remove(0);
        let canonical = parsed.canonical_url(Some(&chosen.reference), Some(&chosen.scope))?;
        resolution_from_candidates(canonical, chosen.commit, chosen.candidates)
    }

    fn download_archive(
        &self,
        repository: &GitHubRepository,
        commit: &GitHubCommit,
    ) -> Result<Vec<u8>, SkillSourceResolutionError> {
        let archive = self
            .transport
            .download_archive(&GitHubArchiveRequest::new(
                repository.clone(),
                commit.clone(),
            ))
            .map_err(map_archive_transport_error)?;
        if archive.len() > MAX_GITHUB_ZIP_BYTES {
            return Err(repository_too_large());
        }
        Ok(archive)
    }
}

impl SkillInstallationSourceResolver for GitHubInstallationSourceResolver {
    fn id(&self) -> SkillSourceResolverId {
        github_resolver_id()
    }

    fn supported_hosts(&self) -> Vec<String> {
        GITHUB_HOSTS
            .iter()
            .map(|host| (*host).to_string())
            .collect()
    }

    fn resolve(
        &self,
        locator: &SkillInstallationSourceLocator,
    ) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
        self.resolve_github_url(locator)
    }
}

fn github_resolver_id() -> SkillSourceResolverId {
    SkillSourceResolverId::parse(GITHUB_RESOLVER_ID)
        .expect("the built-in GitHub source resolver id must remain valid")
}

#[derive(Debug, Clone)]
struct ParsedGitHubUrl {
    repository: GitHubRepository,
    target: GitHubUrlTarget,
}

#[derive(Debug, Clone)]
enum GitHubUrlTarget {
    Repository,
    Tree { tail: Vec<String> },
    SkillFile { tail: Vec<String> },
}

impl ParsedGitHubUrl {
    fn parse(value: &str) -> Result<Self, SkillSourceResolutionError> {
        let url = Url::parse(value).map_err(|_| {
            SkillSourceResolutionError::parse(
                SkillSourceResolutionErrorCode::InvalidLocator,
                SkillSourceResolutionRecovery::FixLocator,
                "The GitHub Skill URL is invalid.",
            )
        })?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some_and(|port| port != 443)
        {
            return Err(SkillSourceResolutionError::parse(
                SkillSourceResolutionErrorCode::InvalidLocator,
                SkillSourceResolutionRecovery::FixLocator,
                "GitHub Skill URLs must use HTTPS and cannot contain credentials or a custom port.",
            ));
        }
        let host = url.host_str().unwrap_or_default();
        let components = decode_path_components(url.path())?;
        if components.len() > MAX_GITHUB_URL_COMPONENTS {
            return Err(invalid_url_shape("The GitHub URL path is too deep."));
        }

        let (owner, repository, target) = match host {
            "github.com" | "www.github.com" => parse_github_page_components(components)?,
            "raw.githubusercontent.com" => parse_raw_components(components)?,
            _ => {
                return Err(SkillSourceResolutionError::parse(
                    SkillSourceResolutionErrorCode::UnsupportedHost,
                    SkillSourceResolutionRecovery::ChooseDifferentSource,
                    "This GitHub Skill URL host is not supported.",
                ));
            }
        };
        let repository = repository.strip_suffix(".git").unwrap_or(&repository);
        let repository =
            GitHubRepository::parse(owner.to_ascii_lowercase(), repository.to_ascii_lowercase())
                .map_err(|_| {
                    invalid_url_shape(
                        "The GitHub URL does not contain valid repository coordinates.",
                    )
                })?;
        Ok(Self { repository, target })
    }

    fn canonical_url(
        &self,
        reference: Option<&str>,
        scope: Option<&GitHubSubdirectory>,
    ) -> Result<String, SkillSourceResolutionError> {
        let mut url = Url::parse("https://github.com/").map_err(|_| unavailable())?;
        {
            let mut path = url.path_segments_mut().map_err(|_| unavailable())?;
            path.pop_if_empty();
            path.push(self.repository.owner());
            path.push(self.repository.name());
            match (&self.target, reference, scope) {
                (GitHubUrlTarget::Repository, _, _) => {}
                (GitHubUrlTarget::Tree { .. }, Some(reference), Some(scope)) => {
                    path.push("tree");
                    for component in reference.split('/') {
                        path.push(component);
                    }
                    if !scope.as_str().is_empty() {
                        for component in scope.as_str().split('/') {
                            path.push(component);
                        }
                    }
                }
                (GitHubUrlTarget::SkillFile { .. }, Some(reference), Some(scope)) => {
                    path.push("blob");
                    for component in reference.split('/') {
                        path.push(component);
                    }
                    if !scope.as_str().is_empty() {
                        for component in scope.as_str().split('/') {
                            path.push(component);
                        }
                    }
                    path.push(SKILL_FILE_NAME);
                }
                _ => return Err(unavailable()),
            }
        }
        Ok(url.into())
    }
}

fn parse_github_page_components(
    components: Vec<String>,
) -> Result<(String, String, GitHubUrlTarget), SkillSourceResolutionError> {
    if components.len() < 2 {
        return Err(invalid_url_shape(
            "Paste a GitHub repository, Skill directory, or SKILL.md URL.",
        ));
    }
    let owner = components[0].clone();
    let repository = components[1].clone();
    if components.len() == 2 {
        return Ok((owner, repository, GitHubUrlTarget::Repository));
    }
    if components.len() < 4 {
        return Err(invalid_url_shape(
            "The GitHub URL does not identify a repository ref.",
        ));
    }
    let route = components[2].as_str();
    let tail = components[3..].to_vec();
    let target = match route {
        "tree" => GitHubUrlTarget::Tree { tail },
        "blob" if tail.last().is_some_and(|value| value == SKILL_FILE_NAME) => {
            GitHubUrlTarget::SkillFile { tail }
        }
        "blob" => {
            return Err(invalid_url_shape(
                "A GitHub file URL must point to an exact-case SKILL.md file.",
            ));
        }
        _ => {
            return Err(invalid_url_shape(
                "Paste a GitHub repository, Skill directory, or SKILL.md URL.",
            ));
        }
    };
    Ok((owner, repository, target))
}

fn parse_raw_components(
    components: Vec<String>,
) -> Result<(String, String, GitHubUrlTarget), SkillSourceResolutionError> {
    if components.len() < 4
        || components
            .last()
            .is_none_or(|value| value != SKILL_FILE_NAME)
    {
        return Err(invalid_url_shape(
            "A raw GitHub URL must point to an exact-case SKILL.md file.",
        ));
    }
    Ok((
        components[0].clone(),
        components[1].clone(),
        GitHubUrlTarget::SkillFile {
            tail: components[2..].to_vec(),
        },
    ))
}

fn decode_path_components(path: &str) -> Result<Vec<String>, SkillSourceResolutionError> {
    let raw = path.strip_prefix('/').unwrap_or(path);
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let mut parts = raw.split('/').collect::<Vec<_>>();
    if parts.last() == Some(&"") {
        parts.pop();
    }
    if parts.iter().any(|component| component.is_empty()) {
        return Err(invalid_url_shape(
            "The GitHub URL contains an empty path component.",
        ));
    }
    parts.into_iter().map(decode_path_component).collect()
}

fn decode_path_component(value: &str) -> Result<String, SkillSourceResolutionError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(invalid_url_shape(
                    "The GitHub URL contains invalid percent encoding.",
                ));
            }
            let high = hex_value(bytes[index + 1]).ok_or_else(|| {
                invalid_url_shape("The GitHub URL contains invalid percent encoding.")
            })?;
            let low = hex_value(bytes[index + 2]).ok_or_else(|| {
                invalid_url_shape("The GitHub URL contains invalid percent encoding.")
            })?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(decoded)
        .map_err(|_| invalid_url_shape("The GitHub URL path is not valid UTF-8."))?;
    if decoded.is_empty()
        || decoded.len() > 255
        || matches!(decoded.as_str(), "." | "..")
        || decoded.contains(['/', '\\', '\0'])
        || decoded.chars().any(char::is_control)
    {
        return Err(invalid_url_shape(
            "The GitHub URL contains an unsafe path component.",
        ));
    }
    Ok(decoded)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Debug)]
struct ReferenceInterpretation {
    reference: String,
    scope: GitHubSubdirectory,
}

fn reference_interpretations(
    tail: &[String],
    direct_skill: bool,
) -> Result<Vec<ReferenceInterpretation>, SkillSourceResolutionError> {
    if tail.is_empty() {
        return Err(invalid_url_shape(
            "The GitHub URL does not contain a branch, tag, or commit.",
        ));
    }
    let max_split = if direct_skill {
        tail.len().saturating_sub(1)
    } else {
        tail.len()
    };
    if max_split == 0 {
        return Err(invalid_url_shape(
            "The GitHub URL does not contain a valid Skill directory path.",
        ));
    }
    if is_full_sha(&tail[0]) {
        return build_interpretation(tail, 1, direct_skill).map(|value| vec![value]);
    }
    // Ref names may contain slashes, so try a bounded number of left-to-right splits. Limit the
    // number of network lookups, not the directory depth: a normal `main/a/very/deep/path` URL
    // must not be rejected merely because the selected Skill lives deeply in the repository.
    let interpretations = (1..=max_split.min(MAX_GITHUB_REF_INTERPRETATIONS))
        .filter_map(|split| build_interpretation(tail, split, direct_skill).ok())
        .collect::<Vec<_>>();
    if interpretations.is_empty() {
        return Err(invalid_url_shape(
            "The GitHub URL does not contain a valid branch and directory combination.",
        ));
    }
    Ok(interpretations)
}

fn build_interpretation(
    tail: &[String],
    split: usize,
    direct_skill: bool,
) -> Result<ReferenceInterpretation, SkillSourceResolutionError> {
    let reference = tail[..split].join("/");
    let mut path = tail[split..].to_vec();
    if direct_skill && path.pop().as_deref() != Some(SKILL_FILE_NAME) {
        return Err(invalid_url_shape(
            "The GitHub file URL must end in exact-case SKILL.md.",
        ));
    }
    let scope = GitHubSubdirectory::parse(path.join("/")).map_err(|_| {
        invalid_url_shape("The GitHub URL contains an invalid Skill directory path.")
    })?;
    Ok(ReferenceInterpretation { reference, scope })
}

fn is_full_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Clone, Copy)]
enum CandidateSelection {
    Discover,
    Exact,
}

#[derive(Debug, Default)]
struct ResolutionPreparationBudget {
    attempts: u64,
    files: u64,
    bytes: u64,
}

impl ResolutionPreparationBudget {
    fn reserve(&mut self, files: u64, bytes: u64) -> Result<(), SkillSourceResolutionError> {
        self.attempts = self
            .attempts
            .checked_add(1)
            .filter(|attempts| *attempts <= MAX_RESOLUTION_CANDIDATE_ATTEMPTS)
            .ok_or_else(repository_too_large)?;
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|total| *total <= MAX_RESOLUTION_SKILL_BYTES)
            .ok_or_else(repository_too_large)?;
        self.files = self
            .files
            .checked_add(files)
            .filter(|total| *total <= MAX_RESOLUTION_SKILL_FILES)
            .ok_or_else(repository_too_large)?;
        Ok(())
    }
}

#[derive(Debug)]
struct ArchiveDiscovery {
    scope_exists: bool,
    subdirectories: Vec<GitHubSubdirectory>,
}

fn discover_archive_skills(
    archive_bytes: &[u8],
    scope: &GitHubSubdirectory,
) -> Result<ArchiveDiscovery, SkillSourceResolutionError> {
    let mut archive = ZipArchive::new(Cursor::new(archive_bytes)).map_err(|_| unsafe_archive())?;
    if archive.is_empty() || archive.len() > MAX_GITHUB_ARCHIVE_ENTRIES {
        return Err(repository_too_large());
    }
    let mut top_root: Option<String> = None;
    let mut scope_exists = scope.as_str().is_empty();
    let mut roots = BTreeSet::<String>::new();

    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|_| unsafe_archive())?;
        let path = validate_archive_entry_path(&entry).map_err(map_archive_error)?;
        let root = path.wrapper_root();
        match &top_root {
            None => top_root = Some(root.to_string()),
            Some(expected) if expected == root => {}
            Some(_) => return Err(unsafe_archive()),
        }
        if !path.is_within(scope) {
            continue;
        }
        scope_exists = true;
        let Some(relative) = path.repository_relative_utf8() else {
            // A non-UTF-8 entry can be ignored for repository-wide discovery. If it belongs to a
            // selected candidate, the stricter package extraction pass rejects that candidate.
            continue;
        };
        if !entry.is_dir()
            && relative
                .last()
                .is_some_and(|value| value == SKILL_FILE_NAME)
        {
            let parent = &relative[..relative.len() - 1];
            roots.insert(parent.join("/"));
        }
    }

    let mut ordered = roots.into_iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        component_count(left)
            .cmp(&component_count(right))
            .then_with(|| left.cmp(right))
    });
    let mut accepted = Vec::<GitHubSubdirectory>::new();
    for candidate in ordered {
        let Ok(candidate) = GitHubSubdirectory::parse(candidate) else {
            // One non-portable Skill root must not hide unrelated healthy candidates.
            continue;
        };
        if accepted
            .iter()
            .any(|parent| is_same_or_descendant(candidate.as_str(), parent.as_str()))
        {
            continue;
        }
        accepted.push(candidate);
        if accepted.len() > MAX_DISCOVERED_SKILLS {
            return Err(SkillSourceResolutionError::discover(
                SkillSourceResolutionErrorCode::TooManySkills,
                SkillSourceResolutionRecovery::NarrowLocator,
                "This GitHub URL contains too many Skill candidates. Paste a narrower directory URL.",
            ));
        }
    }
    Ok(ArchiveDiscovery {
        scope_exists,
        subdirectories: accepted,
    })
}

fn component_count(path: &str) -> usize {
    if path.is_empty() {
        0
    } else {
        path.split('/').count()
    }
}

fn is_same_or_descendant(candidate: &str, parent: &str) -> bool {
    parent.is_empty()
        || candidate == parent
        || candidate
            .strip_prefix(parent)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn prepare_candidates(
    archive: &[u8],
    repository: &GitHubRepository,
    commit: &GitHubCommit,
    scope: &GitHubSubdirectory,
    selection: CandidateSelection,
    budget: &mut ResolutionPreparationBudget,
) -> Result<Vec<SkillSourceResolutionCandidate>, SkillSourceResolutionError> {
    let discovery = discover_archive_skills(archive, scope)?;
    if !discovery.scope_exists {
        return Err(SkillSourceResolutionError::discover(
            SkillSourceResolutionErrorCode::PathNotFound,
            SkillSourceResolutionRecovery::FixLocator,
            "The GitHub URL directory was not found.",
        ));
    }
    let roots = match selection {
        CandidateSelection::Discover => discovery.subdirectories,
        CandidateSelection::Exact => discovery
            .subdirectories
            .into_iter()
            .filter(|candidate| candidate == scope)
            .collect(),
    };
    if roots.is_empty() {
        return Err(SkillSourceResolutionError::discover(
            SkillSourceResolutionErrorCode::NoSkillsFound,
            SkillSourceResolutionRecovery::ChooseDifferentSource,
            "No exact-case SKILL.md was found at this GitHub URL.",
        ));
    }

    let mut candidates = Vec::new();
    let mut first_rejection = None;
    for subdirectory in roots {
        let (declared_files, declared_bytes) = declared_candidate_cost(archive, &subdirectory)?;
        budget.reserve(declared_files, declared_bytes)?;
        match prepare_candidate(archive, repository, commit, &subdirectory) {
            Ok(candidate) => candidates.push(candidate),
            Err(error) => {
                if matches!(selection, CandidateSelection::Exact) {
                    return Err(error);
                }
                first_rejection.get_or_insert(error);
            }
        }
    }
    if candidates.is_empty() {
        return Err(first_rejection.unwrap_or_else(|| {
            SkillSourceResolutionError::discover(
                SkillSourceResolutionErrorCode::InvalidPackage,
                SkillSourceResolutionRecovery::ChooseDifferentSource,
                "The discovered GitHub Skill packages are not compatible.",
            )
        }));
    }
    Ok(candidates)
}

fn declared_candidate_cost(
    archive_bytes: &[u8],
    subdirectory: &GitHubSubdirectory,
) -> Result<(u64, u64), SkillSourceResolutionError> {
    let mut archive = ZipArchive::new(Cursor::new(archive_bytes)).map_err(|_| unsafe_archive())?;
    let mut files = 0_u64;
    let mut bytes = 0_u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|_| unsafe_archive())?;
        let path = validate_archive_entry_path(&entry).map_err(map_archive_error)?;
        if entry.is_dir() || !path.is_within(subdirectory) {
            continue;
        }
        files = files.checked_add(1).ok_or_else(repository_too_large)?;
        bytes = bytes
            .checked_add(entry.size())
            .ok_or_else(repository_too_large)?;
    }
    Ok((files, bytes))
}

fn prepare_candidate(
    archive: &[u8],
    repository: &GitHubRepository,
    commit: &GitHubCommit,
    subdirectory: &GitHubSubdirectory,
) -> Result<SkillSourceResolutionCandidate, SkillSourceResolutionError> {
    let files = extract_selected_skill(archive, subdirectory).map_err(map_archive_error)?;
    let origin = SkillPackageOrigin::new(
        "github",
        format!(
            "{}/{}@{}:{}",
            repository.owner(),
            repository.name(),
            commit.as_str(),
            subdirectory.as_str()
        ),
    )
    .map_err(|_| unavailable())?;
    let package = PreparedSkillPackage::from_files(files, origin).map_err(|_| {
        SkillSourceResolutionError::discover(
            SkillSourceResolutionErrorCode::InvalidPackage,
            SkillSourceResolutionRecovery::ChooseDifferentSource,
            "The discovered GitHub Skill package is invalid or unsupported.",
        )
    })?;
    let resource_index = package.resource_index();
    let resource_bytes = resource_index
        .entries()
        .iter()
        .fold(0_u64, |total, resource| {
            total.saturating_add(resource.byte_length())
        });
    let file_count = u64::try_from(resource_index.len())
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let total_bytes = u64::try_from(package.source_bytes().len())
        .unwrap_or(u64::MAX)
        .saturating_add(resource_bytes);
    let preview = ResolvedSkillPackagePreview::new(
        package.format_version(),
        package.revision().clone(),
        package.name(),
        package.description(),
        file_count,
        total_bytes,
    );
    let source = ResolvedSkillSource::GitHub {
        owner: repository.owner().to_string(),
        repository: repository.name().to_string(),
        resolved_commit: commit.as_str().to_string(),
        subdirectory: (!subdirectory.as_str().is_empty())
            .then(|| subdirectory.as_str().to_string()),
    };
    let candidate_id = candidate_id(
        repository,
        commit,
        subdirectory,
        package.revision().as_str(),
    );
    Ok(SkillSourceResolutionCandidate::new(
        candidate_id,
        source,
        preview,
    ))
}

fn candidate_id(
    repository: &GitHubRepository,
    commit: &GitHubCommit,
    subdirectory: &GitHubSubdirectory,
    package_revision: &str,
) -> String {
    let mut hasher = Sha256::new();
    for value in [
        "mycopilot-skill-source-candidate-v1",
        repository.owner(),
        repository.name(),
        commit.as_str(),
        subdirectory.as_str(),
        package_revision,
    ] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

#[derive(Debug)]
struct ResolvedInterpretation {
    candidates: Vec<SkillSourceResolutionCandidate>,
    commit: GitHubCommit,
    reference: String,
    scope: GitHubSubdirectory,
}

impl ResolvedInterpretation {
    fn fingerprint(&self) -> (String, Vec<String>) {
        (
            self.commit.as_str().to_string(),
            self.candidates
                .iter()
                .map(|candidate| candidate.candidate_id().to_string())
                .collect(),
        )
    }
}

fn resolution_from_candidates(
    canonical_url: String,
    commit: GitHubCommit,
    candidates: Vec<SkillSourceResolutionCandidate>,
) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
    SkillSourceResolution::new(
        canonical_url,
        github_resolver_id(),
        commit.as_str(),
        candidates,
    )
}

fn map_repository_transport_error(error: GitHubTransportError) -> SkillSourceResolutionError {
    match error {
        GitHubTransportError::NotFound => SkillSourceResolutionError::resolve(
            SkillSourceResolutionErrorCode::RepositoryNotFound,
            SkillSourceResolutionRecovery::FixLocator,
            "The public GitHub repository was not found.",
        ),
        other => map_reference_transport_error(other),
    }
}

fn map_reference_transport_error(error: GitHubTransportError) -> SkillSourceResolutionError {
    match error {
        GitHubTransportError::NotFound => SkillSourceResolutionError::resolve(
            SkillSourceResolutionErrorCode::ReferenceNotFound,
            SkillSourceResolutionRecovery::FixLocator,
            "The GitHub branch, tag, or commit was not found.",
        ),
        GitHubTransportError::RateLimited => SkillSourceResolutionError::resolve(
            SkillSourceResolutionErrorCode::RateLimited,
            SkillSourceResolutionRecovery::RetryLater,
            "GitHub temporarily rate-limited this request.",
        ),
        GitHubTransportError::ResponseTooLarge => repository_too_large(),
        GitHubTransportError::Rejected
        | GitHubTransportError::InvalidResponse
        | GitHubTransportError::Unavailable => SkillSourceResolutionError::resolve(
            SkillSourceResolutionErrorCode::NetworkUnavailable,
            SkillSourceResolutionRecovery::RetryLater,
            "GitHub source resolution is temporarily unavailable.",
        ),
    }
}

fn map_archive_transport_error(error: GitHubTransportError) -> SkillSourceResolutionError {
    match error {
        GitHubTransportError::ResponseTooLarge => repository_too_large(),
        GitHubTransportError::NotFound => SkillSourceResolutionError::resolve(
            SkillSourceResolutionErrorCode::RepositoryNotFound,
            SkillSourceResolutionRecovery::FixLocator,
            "The resolved GitHub repository snapshot was not found.",
        ),
        other => map_reference_transport_error(other),
    }
}

fn map_archive_error(error: GitHubAcquisitionError) -> SkillSourceResolutionError {
    match error.code() {
        GitHubAcquisitionErrorCode::ArchiveTooLarge
        | GitHubAcquisitionErrorCode::ArchiveBudgetExceeded => repository_too_large(),
        GitHubAcquisitionErrorCode::InvalidArchive
        | GitHubAcquisitionErrorCode::UnsafeArchiveEntry => unsafe_archive(),
        GitHubAcquisitionErrorCode::SkillNotFound => SkillSourceResolutionError::discover(
            SkillSourceResolutionErrorCode::NoSkillsFound,
            SkillSourceResolutionRecovery::ChooseDifferentSource,
            "No exact-case SKILL.md was found at this GitHub URL.",
        ),
        GitHubAcquisitionErrorCode::PackageRejected => SkillSourceResolutionError::discover(
            SkillSourceResolutionErrorCode::InvalidPackage,
            SkillSourceResolutionRecovery::ChooseDifferentSource,
            "The discovered GitHub Skill package is invalid or unsupported.",
        ),
        _ => unavailable(),
    }
}

fn invalid_url_shape(message: &'static str) -> SkillSourceResolutionError {
    SkillSourceResolutionError::parse(
        SkillSourceResolutionErrorCode::UnsupportedUrlShape,
        SkillSourceResolutionRecovery::FixLocator,
        message,
    )
}

fn repository_too_large() -> SkillSourceResolutionError {
    SkillSourceResolutionError::discover(
        SkillSourceResolutionErrorCode::RepositoryTooLarge,
        SkillSourceResolutionRecovery::NarrowLocator,
        "The GitHub repository snapshot exceeds the source-resolution limits.",
    )
}

fn unsafe_archive() -> SkillSourceResolutionError {
    SkillSourceResolutionError::discover(
        SkillSourceResolutionErrorCode::UnsafePackage,
        SkillSourceResolutionRecovery::ChooseDifferentSource,
        "The selected GitHub Skill package failed archive safety validation.",
    )
}

fn unavailable() -> SkillSourceResolutionError {
    SkillSourceResolutionError::resolve(
        SkillSourceResolutionErrorCode::Unavailable,
        SkillSourceResolutionRecovery::RetryLater,
        "Skill source resolution is temporarily unavailable.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use std::sync::Mutex;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    const COMMIT_A: &str = "0123456789abcdef0123456789abcdef01234567";
    const COMMIT_B: &str = "89abcdef0123456789abcdef0123456789abcdef";
    const SKILL_A: &[u8] = b"---\nname: alpha\ndescription: Alpha fixture.\n---\n\n# Alpha\n";
    const SKILL_B: &[u8] = b"---\nname: beta\ndescription: Beta fixture.\n---\n\n# Beta\n";

    #[derive(Default)]
    struct FakeTransport {
        archives: BTreeMap<String, Vec<u8>>,
        refs: BTreeMap<String, String>,
        rejected_refs: BTreeSet<String>,
        resolved_refs: Mutex<Vec<String>>,
    }

    impl GitHubAcquisitionTransport for FakeTransport {
        fn resolve_commit(
            &self,
            request: &GitHubResolveRequest,
        ) -> Result<GitHubCommit, GitHubTransportError> {
            let key = match request.reference() {
                GitHubReference::DefaultBranch => "<default>".to_string(),
                GitHubReference::Named(value) => value.as_str().to_string(),
                GitHubReference::Commit(value) => value.as_str().to_string(),
            };
            self.resolved_refs.lock().unwrap().push(key.clone());
            if self.rejected_refs.contains(&key) {
                return Err(GitHubTransportError::Rejected);
            }
            self.refs
                .get(&key)
                .or_else(|| is_full_sha(&key).then_some(&key))
                .ok_or(GitHubTransportError::NotFound)
                .and_then(|commit| {
                    GitHubCommit::parse(commit.clone())
                        .map_err(|_| GitHubTransportError::InvalidResponse)
                })
        }

        fn download_archive(
            &self,
            request: &GitHubArchiveRequest,
        ) -> Result<Vec<u8>, GitHubTransportError> {
            self.archives
                .get(request.commit().as_str())
                .cloned()
                .ok_or(GitHubTransportError::NotFound)
        }
    }

    fn resolver(transport: FakeTransport) -> GitHubInstallationSourceResolver {
        GitHubInstallationSourceResolver::new(Arc::new(transport))
    }

    fn transport(commit: &str, files: &[(&str, &[u8])]) -> FakeTransport {
        FakeTransport {
            archives: BTreeMap::from([(commit.to_string(), write_zip(files))]),
            refs: BTreeMap::from([
                ("<default>".to_string(), commit.to_string()),
                ("main".to_string(), commit.to_string()),
            ]),
            rejected_refs: BTreeSet::new(),
            resolved_refs: Mutex::new(Vec::new()),
        }
    }

    fn resolve(
        resolver: &GitHubInstallationSourceResolver,
        url: &str,
    ) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
        resolver.resolve(&SkillInstallationSourceLocator::url(url).unwrap())
    }

    #[test]
    fn repository_url_discovers_multiple_valid_skills_and_pins_the_commit() {
        let resolver = resolver(transport(
            COMMIT_A,
            &[
                ("root/skills/alpha/SKILL.md", SKILL_A),
                ("root/skills/alpha/LICENSE", b"MIT"),
                ("root/skills/beta/SKILL.md", SKILL_B),
            ],
        ));

        let result = resolve(
            &resolver,
            "https://github.com/example/skills?tab=readme#top",
        )
        .unwrap();

        assert_eq!(result.resolved_revision(), COMMIT_A);
        assert_eq!(result.candidates().len(), 2);
        assert_eq!(
            result.outcome(),
            super::super::source_resolution::SkillSourceResolutionOutcome::SelectionRequired
        );
        assert_eq!(result.canonical_url(), "https://github.com/example/skills");
        assert_eq!(result.candidates()[0].package().name(), "alpha");
        assert_eq!(result.candidates()[1].package().name(), "beta");
        for candidate in result.candidates() {
            let ResolvedSkillSource::GitHub {
                resolved_commit, ..
            } = candidate.source();
            assert_eq!(resolved_commit, COMMIT_A);
            assert_eq!(candidate.candidate_id().len(), 64);
        }
    }

    #[test]
    fn repository_discovery_isolates_nonportable_candidate_roots() {
        let resolver = resolver(transport(
            COMMIT_A,
            &[
                ("root/bad:/SKILL.md", SKILL_B),
                ("root/skills/alpha/SKILL.md", SKILL_A),
            ],
        ));

        let result = resolve(&resolver, "https://github.com/example/skills").unwrap();

        assert_eq!(result.candidates().len(), 1);
        assert_eq!(result.candidates()[0].package().name(), "alpha");
    }

    #[test]
    fn equivalent_repository_case_normalizes_to_one_candidate_identity() {
        let lower = resolve(
            &resolver(transport(
                COMMIT_A,
                &[("root/skills/alpha/SKILL.md", SKILL_A)],
            )),
            "https://github.com/example/skills/tree/main/skills/alpha",
        )
        .unwrap();
        let mixed = resolve(
            &resolver(transport(
                COMMIT_A,
                &[("root/skills/alpha/SKILL.md", SKILL_A)],
            )),
            "https://github.com/Example/Skills/tree/main/skills/alpha",
        )
        .unwrap();

        assert_eq!(lower.canonical_url(), mixed.canonical_url());
        assert_eq!(
            lower.candidates()[0].candidate_id(),
            mixed.candidates()[0].candidate_id()
        );
    }

    #[test]
    fn preparation_budget_charges_declared_work_before_package_validation() {
        let archive = write_zip(&[
            ("root/invalid/SKILL.md", b"not a valid skill"),
            ("root/invalid/README.md", b"still charged"),
        ]);
        let subdirectory = GitHubSubdirectory::parse("invalid").unwrap();
        let (files, bytes) = declared_candidate_cost(&archive, &subdirectory).unwrap();
        let mut budget = ResolutionPreparationBudget::default();

        budget.reserve(files, bytes).unwrap();

        assert_eq!(budget.attempts, 1);
        assert_eq!(budget.files, 2);
        assert_eq!(
            budget.bytes,
            u64::try_from(b"not a valid skill".len() + b"still charged".len()).unwrap()
        );
    }

    #[test]
    fn tree_blob_and_raw_urls_resolve_to_the_same_exact_skill() {
        let urls = [
            "https://github.com/example/skills/tree/main/skills/alpha",
            "https://github.com/example/skills/blob/main/skills/alpha/SKILL.md?plain=1#L1",
            "https://raw.githubusercontent.com/example/skills/main/skills/alpha/SKILL.md",
        ];
        for url in urls {
            let resolver = resolver(transport(
                COMMIT_A,
                &[("root/skills/alpha/SKILL.md", SKILL_A)],
            ));
            let result = resolve(&resolver, url).unwrap();
            assert_eq!(result.candidates().len(), 1, "{url}");
            let ResolvedSkillSource::GitHub { subdirectory, .. } = result.candidates()[0].source();
            assert_eq!(subdirectory.as_deref(), Some("skills/alpha"), "{url}");
        }
    }

    #[test]
    fn explicit_ref_urls_do_not_depend_on_the_repository_default_branch() {
        let mut fake = transport(COMMIT_A, &[("root/skills/alpha/SKILL.md", SKILL_A)]);
        fake.refs.remove("<default>");
        let resolver = resolver(fake);

        let result = resolve(
            &resolver,
            "https://github.com/example/skills/tree/main/skills/alpha",
        )
        .unwrap();

        assert_eq!(result.resolved_revision(), COMMIT_A);
        assert_eq!(result.candidates().len(), 1);
    }

    #[test]
    fn rejected_longer_pseudo_refs_do_not_poison_a_valid_shorter_ref() {
        let mut fake = transport(COMMIT_A, &[("root/skills/alpha/SKILL.md", SKILL_A)]);
        fake.rejected_refs.insert("main/skills".to_string());
        let resolver = resolver(fake);

        let result = resolve(
            &resolver,
            "https://github.com/example/skills/tree/main/skills/alpha",
        )
        .unwrap();

        assert_eq!(result.candidates().len(), 1);
    }

    #[test]
    fn immutable_permalink_rejects_a_transport_commit_mismatch() {
        let mut fake = transport(COMMIT_A, &[("root/skills/alpha/SKILL.md", SKILL_A)]);
        fake.refs.insert(COMMIT_B.to_string(), COMMIT_A.to_string());
        let resolver = resolver(fake);

        let error = resolve(
            &resolver,
            &format!("https://github.com/example/skills/tree/{COMMIT_B}/skills/alpha"),
        )
        .unwrap_err();

        assert_eq!(error.code(), SkillSourceResolutionErrorCode::Unavailable);
    }

    #[test]
    fn slash_branch_is_resolved_by_bounded_server_side_interpretation() {
        let mut fake = transport(COMMIT_A, &[("root/skills/alpha/SKILL.md", SKILL_A)]);
        fake.refs.remove("main");
        fake.refs
            .insert("feature/foo".to_string(), COMMIT_A.to_string());
        let resolver = resolver(fake);

        let result = resolve(
            &resolver,
            "https://github.com/example/skills/tree/feature/foo/skills/alpha",
        )
        .unwrap();

        assert_eq!(result.candidates().len(), 1);
        assert!(result.canonical_url().contains("feature/foo/skills/alpha"));
    }

    #[test]
    fn deeply_nested_skill_paths_do_not_expand_the_ref_lookup_budget() {
        let path = (1..=20)
            .map(|index| format!("level-{index}"))
            .collect::<Vec<_>>()
            .join("/");
        let archive_path = format!("root/{path}/SKILL.md");
        let resolver = resolver(transport(COMMIT_A, &[(archive_path.as_str(), SKILL_A)]));

        let result = resolve(
            &resolver,
            &format!("https://github.com/example/skills/tree/main/{path}"),
        )
        .unwrap();

        assert_eq!(result.candidates().len(), 1);
        let ResolvedSkillSource::GitHub { subdirectory, .. } = result.candidates()[0].source();
        assert_eq!(subdirectory.as_deref(), Some(path.as_str()));
    }

    #[test]
    fn path_components_that_are_invalid_git_refs_do_not_poison_a_valid_shorter_ref() {
        let path = "skills/.hidden/foo.lock";
        let resolver = resolver(transport(
            COMMIT_A,
            &[("root/skills/.hidden/foo.lock/SKILL.md", SKILL_A)],
        ));

        let result = resolve(
            &resolver,
            &format!("https://github.com/example/skills/tree/main/{path}"),
        )
        .unwrap();

        assert_eq!(result.candidates().len(), 1);
        let ResolvedSkillSource::GitHub { subdirectory, .. } = result.candidates()[0].source();
        assert_eq!(subdirectory.as_deref(), Some(path));
    }

    #[test]
    fn differing_valid_ref_path_splits_are_rejected_as_ambiguous() {
        let mut fake = FakeTransport::default();
        fake.refs
            .insert("<default>".to_string(), COMMIT_A.to_string());
        fake.refs
            .insert("feature".to_string(), COMMIT_A.to_string());
        fake.refs
            .insert("feature/foo".to_string(), COMMIT_B.to_string());
        fake.archives.insert(
            COMMIT_A.to_string(),
            write_zip(&[("root/foo/skills/alpha/SKILL.md", SKILL_A)]),
        );
        fake.archives.insert(
            COMMIT_B.to_string(),
            write_zip(&[("root/skills/alpha/SKILL.md", SKILL_A)]),
        );
        let resolver = resolver(fake);

        let error = resolve(
            &resolver,
            "https://github.com/example/skills/tree/feature/foo/skills/alpha",
        )
        .unwrap_err();

        assert_eq!(
            error.code(),
            SkillSourceResolutionErrorCode::AmbiguousReference
        );
    }

    #[test]
    fn unsupported_pages_and_encoded_path_separators_are_rejected_before_network() {
        let resolver = resolver(FakeTransport::default());
        for url in [
            "https://github.com/example/skills/issues/1",
            "https://github.com/example/skills/blob/main/README.md",
            "https://github.com/example/skills/tree/main/skills%2Falpha",
        ] {
            assert_eq!(
                resolve(&resolver, url).unwrap_err().phase(),
                super::super::source_resolution::SkillSourceResolutionPhase::Parse,
                "{url}",
            );
        }
    }

    fn write_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut cursor);
            for (path, contents) in files {
                writer
                    .start_file(*path, SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(contents).unwrap();
            }
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }
}
