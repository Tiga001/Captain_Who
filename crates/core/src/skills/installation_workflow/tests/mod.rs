use super::super::origin::SkillPackageOrigin;
use super::super::{
    GitHubAcquisitionSummary, GitHubCommit, GitHubReference, GitHubRepository, GitHubSubdirectory,
    PreparedSkillSourceResolution, PreparedSkillSourceResolutionCandidate, ResolvedSkillSource,
    SkillInstallationOutcome, SkillInstallationSourceLocator, SkillInstallationSourceResolver,
    SkillSourceResolution, SkillSourceResolutionError, SkillSourceResolutionService,
    SkillSourceResolverId, SkillsService,
};
use super::*;
use std::fs;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tempfile::tempdir;

mod acquisition;
mod preview_commit;
mod recovery;

const INSTALLATION_ID: &str = "01234567-89ab-4def-8123-456789abcdef";

fn installation_id() -> SkillInstallationId {
    SkillInstallationId::parse(INSTALLATION_ID).unwrap()
}

fn write_skill(directory: &Path, marker: &str, with_resources: bool) {
    fs::create_dir_all(directory).unwrap();
    fs::write(
        directory.join("SKILL.md"),
        format!(
            "---\nname: workflow-fixture\ndescription: Workflow fixture.\n---\n# Instructions\n{marker}\n"
        ),
    )
    .unwrap();
    if with_resources {
        fs::create_dir_all(directory.join("references")).unwrap();
        fs::create_dir_all(directory.join("assets")).unwrap();
        fs::create_dir_all(directory.join("scripts")).unwrap();
        fs::write(directory.join("references/guide.md"), "guide").unwrap();
        fs::write(directory.join("assets/icon.bin"), [0_u8, 1, 2]).unwrap();
        fs::write(directory.join("scripts/check.sh"), "echo check\n").unwrap();
    }
}

fn workflow(store: &Path) -> SkillInstallationWorkflow {
    SkillInstallationWorkflow::new(SkillInstallationService::new(store).unwrap())
}

const REFRESH_FIXTURE_PROVIDER: &str = "fixture-refresh";

fn provider_acquisition(
    marker: &str,
    provider: &str,
    authority_payload: &str,
    refresh: Option<(&str, u32, &str)>,
) -> PreparedSkillAcquisition {
    let package = PreparedSkillPackage::from_bytes(
        format!(
            "---\nname: refresh-fixture\ndescription: Refresh fixture.\n---\n# Instructions\n{marker}\n"
        )
        .into_bytes(),
        SkillPackageOrigin::new(provider, authority_payload).unwrap(),
    )
    .unwrap();
    let authority = SkillInstallationAuthority::new(provider, 1, authority_payload).unwrap();
    let refresh = refresh.map(|(provider, schema_version, payload)| {
        SkillInstallationRefresh::new(provider, schema_version, payload).unwrap()
    });
    PreparedSkillAcquisition::new(
        package,
        SkillInstallationProvenance::new(authority, refresh),
    )
}

fn fixture_acquisition(
    marker: &str,
    authority_payload: &str,
    refresh: Option<(&str, u32, &str)>,
) -> PreparedSkillAcquisition {
    provider_acquisition(marker, REFRESH_FIXTURE_PROVIDER, authority_payload, refresh)
}

fn acquisition_with_provider_parts(
    marker: &str,
    origin_provider: &str,
    authority_provider: &str,
    refresh_provider: Option<&str>,
) -> PreparedSkillAcquisition {
    let package = PreparedSkillPackage::from_bytes(
        format!(
            "---\nname: provider-binding-fixture\ndescription: Provider binding fixture.\n---\n# Instructions\n{marker}\n"
        )
        .into_bytes(),
        SkillPackageOrigin::new(origin_provider, "origin").unwrap(),
    )
    .unwrap();
    let authority = SkillInstallationAuthority::new(authority_provider, 1, "authority").unwrap();
    let refresh = refresh_provider
        .map(|provider| SkillInstallationRefresh::new(provider, 1, "tracking").unwrap());
    PreparedSkillAcquisition::new(
        package,
        SkillInstallationProvenance::new(authority, refresh),
    )
}

struct RefreshFixtureAdapter {
    calls: Arc<AtomicUsize>,
    next: PreparedSkillAcquisition,
}

impl SkillAcquisitionAdapter for RefreshFixtureAdapter {
    fn provider(&self) -> SkillAcquisitionProvider {
        SkillAcquisitionProvider::parse(REFRESH_FIXTURE_PROVIDER).unwrap()
    }

    fn acquire(
        &self,
        _source: &SkillAcquisitionSource,
    ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.next.clone())
    }

    fn refresh_schema_versions(&self) -> &'static [u32] {
        &[1]
    }

    fn reacquire(
        &self,
        refresh: SkillInstallationRefreshView<'_>,
    ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if refresh.provider() != REFRESH_FIXTURE_PROVIDER
            || refresh.schema_version() != 1
            || refresh.payload() != "tracking"
        {
            return Err(SkillAcquisitionAdapterError::invalid_request(
                "invalid fixture refresh metadata",
            ));
        }
        Ok(self.next.clone())
    }

    fn installed_source_presentation(
        &self,
        provenance: SkillInstallationProvenanceView<'_>,
        refresh_capable: bool,
    ) -> Option<InstalledSkillSourcePresentation> {
        let authority = provenance.authority();
        let refresh = provenance.refresh()?;
        (authority.provider() == REFRESH_FIXTURE_PROVIDER
            && authority.schema_version() == 1
            && refresh.provider() == REFRESH_FIXTURE_PROVIDER
            && refresh.schema_version() == 1
            && refresh.payload() == "tracking")
            .then(|| InstalledSkillSourcePresentation::Provider {
                provider: REFRESH_FIXTURE_PROVIDER.to_string(),
                display_name: "Fixture refresh".to_string(),
                refreshable: refresh_capable,
            })
    }
}

const PRESENTATION_FIXTURE_PROVIDER: &str = "fixture-presentation";

struct PresentationFixtureAdapter {
    presentation: InstalledSkillSourcePresentation,
}

impl SkillAcquisitionAdapter for PresentationFixtureAdapter {
    fn provider(&self) -> SkillAcquisitionProvider {
        SkillAcquisitionProvider::parse(PRESENTATION_FIXTURE_PROVIDER).unwrap()
    }

    fn acquire(
        &self,
        _source: &SkillAcquisitionSource,
    ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
        Err(SkillAcquisitionAdapterError::invalid_request(
            "the presentation fixture does not acquire packages",
        ))
    }

    fn refresh_schema_versions(&self) -> &'static [u32] {
        &[1]
    }

    fn installed_source_presentation(
        &self,
        _provenance: SkillInstallationProvenanceView<'_>,
        _refresh_capable: bool,
    ) -> Option<InstalledSkillSourcePresentation> {
        Some(self.presentation.clone())
    }
}

fn refresh_workflow(
    store: &Path,
    initial: PreparedSkillAcquisition,
    next: PreparedSkillAcquisition,
) -> (
    SkillInstallationWorkflow,
    InstalledSkillRecord,
    Arc<AtomicUsize>,
) {
    let service = SkillInstallationService::new(store).unwrap();
    let (package, provenance) = initial.into_parts();
    service
        .install_prepared_with_provenance(installation_id(), package, provenance)
        .unwrap();
    let record = service
        .read_installed_skill(&installation_id())
        .unwrap()
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut workflow = SkillInstallationWorkflow::new(service);
    workflow
        .register_adapter(Arc::new(RefreshFixtureAdapter {
            calls: Arc::clone(&calls),
            next,
        }))
        .unwrap();
    (workflow, record, calls)
}

const HANDOFF_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

struct HandoffResolver {
    calls: Arc<AtomicUsize>,
}

impl HandoffResolver {
    fn prepared(&self, locator: &SkillInstallationSourceLocator) -> PreparedSkillSourceResolution {
        let repository = GitHubRepository::parse("example", "skills").unwrap();
        let commit = GitHubCommit::parse(HANDOFF_COMMIT).unwrap();
        let subdirectory = GitHubSubdirectory::parse("skills/handoff").unwrap();
        let summary = GitHubAcquisitionSummary::for_resolved_pin(
            &repository,
            &GitHubReference::DefaultBranch,
            &commit,
            &subdirectory,
        );
        let origin = summary.origin().unwrap();
        let package = PreparedSkillPackage::from_bytes(
            b"---\nname: handoff-fixture\ndescription: Exact handoff fixture.\n---\n# Instructions\nRESOLVED_ONCE_EXACT_BYTES\n"
                .to_vec(),
            origin,
        )
        .unwrap();
        PreparedSkillSourceResolution::new(
            locator.as_url(),
            SkillSourceResolverId::parse("github").unwrap(),
            HANDOFF_COMMIT,
            vec![PreparedSkillSourceResolutionCandidate::new(
                SkillSourceCandidateId::parse("handoff").unwrap(),
                ResolvedSkillSource::GitHub {
                    owner: "example".to_string(),
                    repository: "skills".to_string(),
                    tracking_reference: GitHubReference::DefaultBranch,
                    resolved_commit: HANDOFF_COMMIT.to_string(),
                    subdirectory: Some("skills/handoff".to_string()),
                },
                PreparedSkillAcquisition::new(package, summary.provenance().unwrap()),
            )],
        )
        .unwrap()
    }
}

impl SkillInstallationSourceResolver for HandoffResolver {
    fn id(&self) -> SkillSourceResolverId {
        SkillSourceResolverId::parse("github").unwrap()
    }

    fn supported_hosts(&self) -> Vec<String> {
        vec!["github.com".to_string()]
    }

    fn resolve(
        &self,
        locator: &SkillInstallationSourceLocator,
    ) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
        self.prepared(locator).public_resolution()
    }

    fn resolve_prepared(
        &self,
        locator: &SkillInstallationSourceLocator,
    ) -> Result<PreparedSkillSourceResolution, SkillSourceResolutionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.prepared(locator))
    }
}

fn resolved_handoff(
    store: &Path,
) -> (
    SkillInstallationWorkflow,
    SkillSourceResolutionId,
    SkillSourceCandidateId,
    Arc<AtomicUsize>,
) {
    let sessions = SkillInstallationSessionStore::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut resolutions = SkillSourceResolutionService::with_session_store(sessions.clone());
    resolutions
        .register_resolver(Arc::new(HandoffResolver {
            calls: Arc::clone(&calls),
        }))
        .unwrap();
    let resolution_id = SkillSourceResolutionId::new();
    let locator = SkillInstallationSourceLocator::url("https://github.com/example/skills").unwrap();
    let registered = resolutions
        .resolve_registered(resolution_id.clone(), &locator)
        .unwrap();
    let candidate_id =
        SkillSourceCandidateId::parse(registered.resolution().candidates()[0].candidate_id())
            .unwrap();
    let workflow = SkillInstallationWorkflow::with_session_store(
        SkillInstallationService::new(store).unwrap(),
        SkillInstallationWorkflowConfig::default(),
        sessions,
    );
    (workflow, resolution_id, candidate_id, calls)
}

struct ManualClock {
    elapsed_ms: AtomicU64,
    started_unix_ms: u64,
}

impl Default for ManualClock {
    fn default() -> Self {
        Self {
            elapsed_ms: AtomicU64::new(0),
            started_unix_ms: 1_700_000_000_000,
        }
    }
}

impl ManualClock {
    fn advance(&self, duration: Duration) {
        self.elapsed_ms.fetch_add(
            u64::try_from(duration.as_millis()).unwrap(),
            Ordering::SeqCst,
        );
    }
}

impl SessionClock for ManualClock {
    fn now(&self) -> super::super::installation_session::SessionTime {
        let elapsed_ms = self.elapsed_ms.load(Ordering::SeqCst);
        super::super::installation_session::SessionTime {
            monotonic: Duration::from_millis(elapsed_ms),
            unix_ms: self.started_unix_ms.saturating_add(elapsed_ms),
        }
    }
}
