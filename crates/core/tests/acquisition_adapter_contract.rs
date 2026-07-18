use mycopilot_core::skills::{
    InstalledSkillSourcePresentation, PreparedSkillAcquisition, PreparedSkillPackage,
    SkillAcquisitionAdapter, SkillAcquisitionAdapterError, SkillAcquisitionProvider,
    SkillAcquisitionSource, SkillInstallationAuthority, SkillInstallationCommitRequest,
    SkillInstallationId, SkillInstallationPreparationRequest, SkillInstallationProvenance,
    SkillInstallationProvenanceView, SkillInstallationRefresh, SkillInstallationRefreshView,
    SkillInstallationService, SkillInstallationWorkflow, SkillPackageOrigin, SkillPreparationId,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const PROVIDER: &str = "external-fixture";
const AUTHORITY_V1: &str = "authority-v1";
const AUTHORITY_V2: &str = "authority-v2";
const REFRESH: &str = "track-main";

struct ExternalAdapter {
    next: PreparedSkillAcquisition,
    reacquire_calls: Arc<AtomicUsize>,
    presentation_calls: Arc<AtomicUsize>,
}

impl SkillAcquisitionAdapter for ExternalAdapter {
    fn provider(&self) -> SkillAcquisitionProvider {
        SkillAcquisitionProvider::parse(PROVIDER).unwrap()
    }

    fn acquire(
        &self,
        _source: &SkillAcquisitionSource,
    ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
        Err(SkillAcquisitionAdapterError::invalid_request(
            "the fixture supports installed-source refresh only",
        ))
    }

    fn refresh_schema_versions(&self) -> &'static [u32] {
        &[1]
    }

    fn reacquire(
        &self,
        refresh: SkillInstallationRefreshView<'_>,
    ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
        assert_eq!(refresh.provider(), PROVIDER);
        assert_eq!(refresh.schema_version(), 1);
        assert_eq!(refresh.payload(), REFRESH);
        assert!(!format!("{refresh:?}").contains(REFRESH));
        self.reacquire_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.next.clone())
    }

    fn installed_source_presentation(
        &self,
        provenance: SkillInstallationProvenanceView<'_>,
        refresh_capable: bool,
    ) -> Option<InstalledSkillSourcePresentation> {
        let authority = provenance.authority();
        let refresh = provenance.refresh()?;
        assert_eq!(authority.provider(), PROVIDER);
        assert_eq!(authority.schema_version(), 1);
        assert!(matches!(authority.payload(), AUTHORITY_V1 | AUTHORITY_V2));
        assert_eq!(refresh.provider(), PROVIDER);
        assert_eq!(refresh.schema_version(), 1);
        assert_eq!(refresh.payload(), REFRESH);
        assert!(!format!("{provenance:?}").contains(authority.payload()));
        assert!(!format!("{provenance:?}").contains(refresh.payload()));
        self.presentation_calls.fetch_add(1, Ordering::SeqCst);
        Some(InstalledSkillSourcePresentation::Provider {
            provider: PROVIDER.to_string(),
            display_name: "External fixture".to_string(),
            refreshable: refresh_capable,
        })
    }
}

#[test]
fn external_adapter_can_decode_callback_scoped_provenance_views() {
    let fixture = tempfile::tempdir().unwrap();
    let store = fixture.path().join("store");
    let installation_id = SkillInstallationId::new();
    let initial = acquisition("INITIAL", AUTHORITY_V1);
    let next = acquisition("UPDATED", AUTHORITY_V2);

    let installation_service = SkillInstallationService::new(&store).unwrap();
    installation_service
        .install_prepared_with_provenance(
            installation_id.clone(),
            initial.package().clone(),
            initial.provenance().clone(),
        )
        .unwrap();
    let installed = installation_service
        .read_installed_skill(&installation_id)
        .unwrap()
        .unwrap();
    drop(installation_service);

    let reacquire_calls = Arc::new(AtomicUsize::new(0));
    let presentation_calls = Arc::new(AtomicUsize::new(0));
    let mut workflow =
        SkillInstallationWorkflow::new(SkillInstallationService::new(&store).unwrap());
    workflow
        .register_adapter(Arc::new(ExternalAdapter {
            next: next.clone(),
            reacquire_calls: Arc::clone(&reacquire_calls),
            presentation_calls: Arc::clone(&presentation_calls),
        }))
        .unwrap();

    assert!(matches!(
        workflow.installed_source_presentation(installed.provenance()),
        InstalledSkillSourcePresentation::Provider {
            ref provider,
            ref display_name,
            refreshable: true,
        } if provider == PROVIDER && display_name == "External fixture"
    ));

    let preparation_id = SkillPreparationId::new();
    let preview = workflow
        .inspect(&SkillInstallationPreparationRequest::update(
            preparation_id.clone(),
            installed.skill_id().clone(),
            installed.installation_revision().clone(),
            SkillAcquisitionSource::installed_source(),
        ))
        .unwrap();
    workflow
        .commit(&SkillInstallationCommitRequest::new(
            preparation_id,
            preview.preview_revision().clone(),
        ))
        .unwrap();

    let updated = SkillInstallationService::new(&store)
        .unwrap()
        .read_installed_skill(&installation_id)
        .unwrap()
        .unwrap();
    assert_eq!(updated.package_revision(), next.package().revision());
    assert!(matches!(
        workflow.installed_source_presentation(updated.provenance()),
        InstalledSkillSourcePresentation::Provider {
            refreshable: true,
            ..
        }
    ));
    assert_eq!(reacquire_calls.load(Ordering::SeqCst), 1);
    assert!(presentation_calls.load(Ordering::SeqCst) >= 3);
}

fn acquisition(marker: &str, authority_payload: &str) -> PreparedSkillAcquisition {
    let package = PreparedSkillPackage::from_bytes(
        format!(
            "---\nname: external-fixture\ndescription: External adapter contract fixture.\n---\n# Instructions\n{marker}\n"
        )
        .into_bytes(),
        SkillPackageOrigin::new(PROVIDER, format!("snapshot-{marker}")).unwrap(),
    )
    .unwrap();
    let authority = SkillInstallationAuthority::new(PROVIDER, 1, authority_payload).unwrap();
    let refresh = SkillInstallationRefresh::new(PROVIDER, 1, REFRESH).unwrap();
    PreparedSkillAcquisition::new(
        package,
        SkillInstallationProvenance::new(authority, Some(refresh)),
    )
}
