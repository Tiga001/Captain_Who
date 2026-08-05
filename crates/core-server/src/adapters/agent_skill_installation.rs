use super::skill_installation_workflow_adapter::workflow_failure;
use super::skill_source_resolution_adapter::resolution_failure;
use mycopilot_core::skills::{
    GitHubReference, ResolvedSkillSource, SkillAcquisitionSource, SkillInstallationId,
    SkillInstallationPreparationRequest, SkillInstallationPreview, SkillInstallationSourceLocator,
    SkillInstallationWorkflow, SkillPreparationId, SkillSourceCandidateId, SkillSourceResolutionId,
    SkillSourceResolutionService,
};
use mycopilot_core::{
    AgentError, AgentResult, AgentSkillInstallationPrepareExecutor,
    AgentSkillInstallationPrepareRequest, AgentSkillInstallationPrepareSource,
};
use mycopilot_protocol_rs::SkillInspectionPhaseDto;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const MAX_CANDIDATE_REFS: usize = 1_024;
const MAX_INSTALL_REFS: usize = 512;

/// Agent-facing facade over the existing source-resolution and immutable preparation workflows.
///
/// The facade retains authority-bearing IDs behind random, conversation-bound references.
/// Model results contain only presentation-safe package metadata and recovery guidance.
pub(crate) struct AgentSkillInstallationInspectionAdapter {
    source_resolution: Arc<SkillSourceResolutionService>,
    workflow: Arc<SkillInstallationWorkflow>,
    refs: Mutex<InspectionRefs>,
}

impl std::fmt::Debug for AgentSkillInstallationInspectionAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentSkillInstallationInspectionAdapter")
            .finish_non_exhaustive()
    }
}

impl AgentSkillInstallationInspectionAdapter {
    pub(crate) fn new(
        source_resolution: Arc<SkillSourceResolutionService>,
        workflow: Arc<SkillInstallationWorkflow>,
    ) -> Self {
        Self {
            source_resolution,
            workflow,
            refs: Mutex::new(InspectionRefs::default()),
        }
    }

    fn prepare_url(
        &self,
        request: &AgentSkillInstallationPrepareRequest,
        url: &str,
    ) -> AgentResult<Value> {
        if let Some(candidate_ref) = request.candidate_ref.as_deref() {
            return self.prepare_selected_candidate(request, url, candidate_ref);
        }

        let locator = match SkillInstallationSourceLocator::url(url.to_string()) {
            Ok(locator) => locator,
            Err(error) => return Ok(invalid_resolution(error)),
        };
        let resolution_id = SkillSourceResolutionId::new();
        let registered = match self
            .source_resolution
            .resolve_registered(resolution_id.clone(), &locator)
        {
            Ok(resolution) => resolution,
            Err(error) => return Ok(invalid_resolution(error)),
        };
        let resolution = registered.resolution();
        let source_digest = source_digest(url);

        if resolution.candidates().len() == 1 {
            let candidate = &resolution.candidates()[0];
            return self.inspect_resolved_candidate(
                request,
                resolution_id,
                SkillSourceCandidateId::parse(candidate.candidate_id().to_string()).map_err(
                    |_| invalid_host_state("The resolved Skill candidate identity is invalid."),
                )?,
                json!({
                    "kind": "github",
                    "url": resolution.canonical_url()
                }),
                resolution.resolved_revision().to_string(),
            );
        }

        let now = now_ms();
        let mut refs = self.refs.lock().unwrap_or_else(|error| error.into_inner());
        refs.prune(now);
        if refs
            .candidate_refs
            .len()
            .saturating_add(resolution.candidates().len())
            > MAX_CANDIDATE_REFS
        {
            return Err(capacity_error());
        }
        let mut candidates = Vec::with_capacity(resolution.candidates().len());
        for candidate in resolution.candidates() {
            let candidate_ref = random_ref("skill_candidate_");
            let candidate_id = SkillSourceCandidateId::parse(candidate.candidate_id().to_string())
                .map_err(|_| {
                    invalid_host_state("The resolved Skill candidate identity is invalid.")
                })?;
            refs.candidate_refs.insert(
                candidate_ref.clone(),
                CandidateBinding {
                    conversation_id: request.conversation_id.clone(),
                    _created_by_run_id: request.run_id.clone(),
                    source_digest: source_digest.clone(),
                    resolution_id: resolution_id.clone(),
                    candidate_id,
                    source_summary: json!({
                        "kind": "github",
                        "url": resolution.canonical_url()
                    }),
                    resolved_revision: resolution.resolved_revision().to_string(),
                    expires_at_unix_ms: registered.expires_at_unix_ms(),
                },
            );
            candidates.push(candidate_projection(candidate_ref, candidate));
        }

        Ok(json!({
            "status": "needsSelection",
            "metadataTrust": "untrusted",
            "sourceSummary": {
                "kind": "github",
                "url": resolution.canonical_url()
            },
            "resolvedRevision": resolution.resolved_revision(),
            "candidates": candidates,
            "recovery": "askUserToChooseCandidate"
        }))
    }

    fn prepare_selected_candidate(
        &self,
        request: &AgentSkillInstallationPrepareRequest,
        source: &str,
        candidate_ref: &str,
    ) -> AgentResult<Value> {
        let now = now_ms();
        let binding = {
            let mut refs = self.refs.lock().unwrap_or_else(|error| error.into_inner());
            refs.prune(now);
            refs.candidate_refs.get(candidate_ref).cloned()
        };
        let Some(binding) = binding else {
            return Ok(invalid_result(
                "candidateRefNotFound",
                "The Skill candidate reference is missing or expired.",
                "inspectSourceAgain",
            ));
        };
        if binding.conversation_id != request.conversation_id
            || binding.source_digest != source_digest(source)
        {
            return Ok(invalid_result(
                "candidateRefMismatch",
                "The Skill candidate reference does not belong to this source and conversation.",
                "inspectSourceAgain",
            ));
        }

        let result = self.inspect_resolved_candidate(
            request,
            binding.resolution_id.clone(),
            binding.candidate_id,
            binding.source_summary,
            binding.resolved_revision,
        );
        if result
            .as_ref()
            .is_ok_and(|value| value["status"] == "ready")
        {
            let mut refs = self.refs.lock().unwrap_or_else(|error| error.into_inner());
            refs.candidate_refs
                .retain(|_, candidate| candidate.resolution_id != binding.resolution_id);
        }
        result
    }

    fn inspect_resolved_candidate(
        &self,
        request: &AgentSkillInstallationPrepareRequest,
        resolution_id: SkillSourceResolutionId,
        candidate_id: SkillSourceCandidateId,
        source_summary: Value,
        resolved_revision: String,
    ) -> AgentResult<Value> {
        let preparation_id = SkillPreparationId::new();
        let install_request = SkillInstallationPreparationRequest::install(
            preparation_id,
            SkillInstallationId::new(),
            SkillAcquisitionSource::resolved_candidate(resolution_id, candidate_id),
        );
        let preview = match self.workflow.inspect(&install_request) {
            Ok(preview) => preview,
            Err(error) => return Ok(invalid_workflow(&error)),
        };
        self.ready_result(request, preview, source_summary, resolved_revision)
    }

    fn prepare_local(
        &self,
        request: &AgentSkillInstallationPrepareRequest,
        directory: &std::path::Path,
        display_path: &str,
    ) -> AgentResult<Value> {
        if request.candidate_ref.is_some() {
            return Ok(invalid_result(
                "candidateRefNotAllowed",
                "Local Skill sources do not accept candidateRef.",
                "removeCandidateRef",
            ));
        }
        let preview = match self.workflow.inspect_local_directory_install(
            SkillPreparationId::new(),
            SkillInstallationId::new(),
            directory,
        ) {
            Ok(preview) => preview,
            Err(error) => return Ok(invalid_workflow(&error)),
        };
        let resolved_revision = preview.package().revision().as_str().to_string();
        self.ready_result(
            request,
            preview,
            json!({
                "kind": "localDirectory",
                "path": display_path
            }),
            resolved_revision,
        )
    }

    fn ready_result(
        &self,
        request: &AgentSkillInstallationPrepareRequest,
        preview: SkillInstallationPreview,
        source_summary: Value,
        resolved_revision: String,
    ) -> AgentResult<Value> {
        let install_ref = random_ref("skill_install_");
        let result = ready_projection(&install_ref, &preview, source_summary, resolved_revision);
        let mut refs = self.refs.lock().unwrap_or_else(|error| error.into_inner());
        refs.prune(now_ms());
        if refs.install_refs.len() >= MAX_INSTALL_REFS {
            return Err(capacity_error());
        }
        refs.install_refs.insert(
            install_ref,
            InstallBinding {
                _conversation_id: request.conversation_id.clone(),
                _created_by_run_id: request.run_id.clone(),
                _preparation_id: preview.preparation_id().clone(),
                _preview_revision: preview.preview_revision().as_str().to_string(),
                expires_at_unix_ms: preview.expires_at_unix_ms(),
            },
        );
        Ok(result)
    }

    #[cfg(test)]
    fn install_binding(&self, install_ref: &str) -> Option<InstallBinding> {
        self.refs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .install_refs
            .get(install_ref)
            .cloned()
    }
}

impl AgentSkillInstallationPrepareExecutor for AgentSkillInstallationInspectionAdapter {
    fn prepare(&self, request: AgentSkillInstallationPrepareRequest) -> AgentResult<Value> {
        match request.source.clone() {
            AgentSkillInstallationPrepareSource::Url { url } => self.prepare_url(&request, &url),
            AgentSkillInstallationPrepareSource::LocalDirectory {
                directory,
                display_path,
            } => self.prepare_local(&request, &directory, &display_path),
        }
    }
}

#[derive(Default)]
struct InspectionRefs {
    candidate_refs: HashMap<String, CandidateBinding>,
    install_refs: HashMap<String, InstallBinding>,
}

impl InspectionRefs {
    fn prune(&mut self, now: u64) {
        self.candidate_refs
            .retain(|_, binding| binding.expires_at_unix_ms > now);
        self.install_refs
            .retain(|_, binding| binding.expires_at_unix_ms > now);
    }
}

#[derive(Clone)]
struct CandidateBinding {
    conversation_id: String,
    _created_by_run_id: String,
    source_digest: String,
    resolution_id: SkillSourceResolutionId,
    candidate_id: SkillSourceCandidateId,
    source_summary: Value,
    resolved_revision: String,
    expires_at_unix_ms: u64,
}

#[derive(Clone)]
struct InstallBinding {
    _conversation_id: String,
    _created_by_run_id: String,
    _preparation_id: SkillPreparationId,
    _preview_revision: String,
    expires_at_unix_ms: u64,
}

fn candidate_projection(
    candidate_ref: String,
    candidate: &mycopilot_core::skills::SkillSourceResolutionCandidate,
) -> Value {
    let source_summary = match candidate.source() {
        ResolvedSkillSource::GitHub {
            owner,
            repository,
            tracking_reference,
            resolved_commit,
            subdirectory,
        } => json!({
            "kind": "githubRepository",
            "repository": format!("{owner}/{repository}"),
            "reference": github_reference(tracking_reference),
            "resolvedRevision": resolved_commit,
            "subdirectory": subdirectory
        }),
        _ => json!({ "kind": "unknown" }),
    };
    json!({
        "candidateRef": candidate_ref,
        "name": candidate.package().name(),
        "description": candidate.package().description(),
        "sourceSummary": source_summary,
        "resolvedRevision": candidate.package().revision().as_str(),
        "fileCount": candidate.package().file_count(),
        "totalBytes": candidate.package().total_bytes()
    })
}

fn github_reference(reference: &GitHubReference) -> Value {
    match reference {
        GitHubReference::DefaultBranch => json!({ "kind": "defaultBranch" }),
        GitHubReference::Named(value) => {
            json!({ "kind": "named", "value": value.as_str() })
        }
        GitHubReference::Commit(commit) => {
            json!({ "kind": "commit", "sha": commit.as_str() })
        }
        _ => json!({ "kind": "unknown" }),
    }
}

fn ready_projection(
    install_ref: &str,
    preview: &SkillInstallationPreview,
    source_summary: Value,
    resolved_revision: String,
) -> Value {
    let resources = preview.package().resources();
    let warnings = preview
        .warnings()
        .iter()
        .map(|warning| {
            json!({
                "code": warning.code().stable_name(),
                "message": warning.message(),
                "requiresAcknowledgement": warning.acknowledgement_required()
            })
        })
        .collect::<Vec<_>>();
    json!({
        "status": "ready",
        "metadataTrust": "untrusted",
        "name": preview.package().name(),
        "description": preview.package().description(),
        "sourceSummary": source_summary,
        "resolvedRevision": resolved_revision,
        "fileCount": u64::try_from(resources.resource_count()).unwrap_or(u64::MAX).saturating_add(1),
        "totalBytes": preview.package().package_bytes(),
        "resourceSummary": {
            "total": resources.resource_count(),
            "references": resources.reference_count(),
            "assets": resources.asset_count(),
            "scripts": resources.script_count(),
            "bytes": resources.resource_bytes()
        },
        "containsScripts": resources.script_count() > 0,
        "warnings": warnings,
        "compatibility": if warnings.is_empty() { "compatible" } else { "compatibleWithWarnings" },
        "installRef": install_ref,
        "recovery": "explainThenRequestInstallationApproval"
    })
}

fn invalid_resolution(error: mycopilot_core::skills::SkillSourceResolutionError) -> Value {
    let data = resolution_failure(error).into_data();
    invalid_from_serializable(&*data)
}

fn invalid_workflow(error: &mycopilot_core::skills::SkillInstallationWorkflowError) -> Value {
    let data = workflow_failure(SkillInspectionPhaseDto::Inspect, error).into_data();
    invalid_from_serializable(&*data)
}

fn invalid_from_serializable(data: &impl serde::Serialize) -> Value {
    let encoded = serde_json::to_value(data).unwrap_or_else(|_| json!({}));
    json!({
        "status": "invalid",
        "error": {
            "code": encoded.get("code").cloned().unwrap_or_else(|| json!("unavailable")),
            "message": encoded.get("message").cloned().unwrap_or_else(|| json!("Skill inspection failed."))
        },
        "recovery": encoded.get("recovery").cloned().unwrap_or_else(|| json!("retryLater"))
    })
}

fn invalid_result(code: &str, message: &str, recovery: &str) -> Value {
    json!({
        "status": "invalid",
        "error": { "code": code, "message": message },
        "recovery": recovery
    })
}

fn source_digest(source: &str) -> String {
    format!("{:x}", Sha256::digest(source.trim().as_bytes()))
}

fn random_ref(prefix: &str) -> String {
    format!("{prefix}{}", Uuid::new_v4().simple())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn invalid_host_state(message: impl Into<String>) -> AgentError {
    AgentError::structured(
        "skill.installation.invalidHostState",
        message,
        json!({
            "type": "skillInstallationInspection",
            "code": "invalidHostState",
            "recovery": "retryLater"
        }),
    )
}

fn capacity_error() -> AgentError {
    AgentError::structured(
        "skill.installation.prepareCapacityExceeded",
        "The Skill installation inspection service is temporarily at capacity.",
        json!({
            "type": "skillInstallationInspection",
            "code": "prepareCapacityExceeded",
            "recovery": "retryLater"
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::skills::{
        PreparedSkillAcquisition, PreparedSkillPackage, PreparedSkillSourceResolution,
        PreparedSkillSourceResolutionCandidate, SkillInstallationAuthority,
        SkillInstallationProvenance, SkillInstallationService, SkillInstallationSessionStore,
        SkillInstallationSourceResolver, SkillInstallationWorkflowConfig, SkillPackageOrigin,
        SkillSourceResolution, SkillSourceResolutionError, SkillSourceResolutionErrorCode,
        SkillSourceResolutionRecovery, SkillSourceResolverId,
    };
    use tempfile::tempdir;

    const FIXTURE_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    struct PreparedGitHubFixtureResolver {
        candidate_count: usize,
    }

    impl PreparedGitHubFixtureResolver {
        fn prepared(
            &self,
            locator: &SkillInstallationSourceLocator,
        ) -> PreparedSkillSourceResolution {
            let provider = SkillSourceResolverId::parse("github").unwrap();
            let candidates = (0..self.candidate_count)
                .map(|index| {
                    let package = PreparedSkillPackage::from_bytes(
                        format!(
                            "---\nname: fixture-{index}\ndescription: Third-party fixture {index}.\n---\n\n# Instructions\nDo fixture work.\n"
                        )
                        .into_bytes(),
                        SkillPackageOrigin::new("github", format!("fixture-{index}")).unwrap(),
                    )
                    .unwrap();
                    let authority = SkillInstallationAuthority::new(
                        package.origin().provider(),
                        1,
                        package.origin().reference(),
                    )
                    .unwrap();
                    let provenance = SkillInstallationProvenance::new(authority, None);
                    PreparedSkillSourceResolutionCandidate::new(
                        SkillSourceCandidateId::parse(format!("fixture-{index}")).unwrap(),
                        ResolvedSkillSource::GitHub {
                            owner: "example".to_string(),
                            repository: "skills".to_string(),
                            tracking_reference: GitHubReference::DefaultBranch,
                            resolved_commit: FIXTURE_COMMIT.to_string(),
                            subdirectory: Some(format!("skills/fixture-{index}")),
                        },
                        PreparedSkillAcquisition::new(package, provenance),
                    )
                })
                .collect();
            PreparedSkillSourceResolution::new(
                locator.as_url(),
                provider,
                FIXTURE_COMMIT,
                candidates,
            )
            .unwrap()
        }
    }

    impl SkillInstallationSourceResolver for PreparedGitHubFixtureResolver {
        fn id(&self) -> SkillSourceResolverId {
            SkillSourceResolverId::parse("github").unwrap()
        }

        fn supported_hosts(&self) -> Vec<String> {
            vec!["github.com".to_string()]
        }

        fn resolve(
            &self,
            _locator: &SkillInstallationSourceLocator,
        ) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
            Err(SkillSourceResolutionError::resolve(
                SkillSourceResolutionErrorCode::Unavailable,
                SkillSourceResolutionRecovery::RetryLater,
                "fixture uses prepared resolution",
            ))
        }

        fn resolve_prepared(
            &self,
            locator: &SkillInstallationSourceLocator,
        ) -> Result<PreparedSkillSourceResolution, SkillSourceResolutionError> {
            Ok(self.prepared(locator))
        }
    }

    fn github_adapter(candidate_count: usize) -> AgentSkillInstallationInspectionAdapter {
        let fixture = tempdir().unwrap();
        let store = fixture.keep().join("store");
        let sessions = SkillInstallationSessionStore::default();
        let mut resolution = SkillSourceResolutionService::with_session_store(sessions.clone());
        resolution
            .register_resolver(Arc::new(PreparedGitHubFixtureResolver { candidate_count }))
            .unwrap();
        let workflow = SkillInstallationWorkflow::with_session_store(
            SkillInstallationService::new(store).unwrap(),
            SkillInstallationWorkflowConfig::default(),
            sessions,
        );
        AgentSkillInstallationInspectionAdapter::new(Arc::new(resolution), Arc::new(workflow))
    }

    #[test]
    fn local_prepare_returns_only_public_metadata_and_retains_private_install_binding() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let source = fixture.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: sample\ndescription: Sample inspection skill.\n---\n\n# Sample\n",
        )
        .unwrap();
        let workflow = Arc::new(SkillInstallationWorkflow::new(
            SkillInstallationService::new(store.clone()).unwrap(),
        ));
        let resolution = Arc::new(SkillSourceResolutionService::with_session_store(
            workflow.session_store(),
        ));
        let adapter = AgentSkillInstallationInspectionAdapter::new(resolution, workflow);
        let result = adapter
            .prepare(AgentSkillInstallationPrepareRequest {
                conversation_id: "conversation-1".to_string(),
                run_id: "run-1".to_string(),
                source: AgentSkillInstallationPrepareSource::LocalDirectory {
                    directory: source.clone(),
                    display_path: "source".to_string(),
                },
                candidate_ref: None,
            })
            .unwrap();

        assert_eq!(result["status"], "ready");
        assert_eq!(result["name"], "sample");
        assert_eq!(result["metadataTrust"], "untrusted");
        assert_eq!(result["sourceSummary"]["path"], "source");
        assert!(result.get("preparationId").is_none());
        assert!(result.get("installationId").is_none());
        assert!(result.get("previewRevision").is_none());
        let install_ref = result["installRef"].as_str().unwrap();
        let binding = adapter.install_binding(install_ref).unwrap();
        assert_eq!(binding._conversation_id, "conversation-1");
        assert_eq!(binding._created_by_run_id, "run-1");
        assert!(!binding._preparation_id.as_str().is_empty());
        assert!(!binding._preview_revision.is_empty());
        assert!(binding.expires_at_unix_ms > now_ms());
        assert!(!source.join("receipt.json").exists());
        let inventory = SkillInstallationService::new(store)
            .unwrap()
            .list_installed_skills()
            .unwrap();
        assert!(inventory.records().is_empty());
    }

    #[test]
    fn exact_github_source_is_pinned_and_prepared_without_exposing_authority_ids() {
        let adapter = github_adapter(1);
        let result = adapter
            .prepare(AgentSkillInstallationPrepareRequest {
                conversation_id: "conversation-1".to_string(),
                run_id: "run-1".to_string(),
                source: AgentSkillInstallationPrepareSource::Url {
                    url: "https://github.com/example/skills/tree/main/skills/fixture-0".to_string(),
                },
                candidate_ref: None,
            })
            .unwrap();

        assert_eq!(result["status"], "ready");
        assert_eq!(result["name"], "fixture-0");
        assert_eq!(result["resolvedRevision"], FIXTURE_COMMIT);
        assert_eq!(result["metadataTrust"], "untrusted");
        assert!(result["installRef"]
            .as_str()
            .unwrap()
            .starts_with("skill_install_"));
        for hidden in [
            "preparationId",
            "installationId",
            "previewRevision",
            "temporaryDirectory",
            "session",
        ] {
            assert!(result.get(hidden).is_none(), "leaked {hidden}");
        }
    }

    #[test]
    fn multiple_github_candidates_require_selection_and_refs_cross_user_runs_only_in_conversation()
    {
        let adapter = github_adapter(2);
        let source = "https://github.com/example/skills";
        let result = adapter
            .prepare(AgentSkillInstallationPrepareRequest {
                conversation_id: "conversation-1".to_string(),
                run_id: "run-1".to_string(),
                source: AgentSkillInstallationPrepareSource::Url {
                    url: source.to_string(),
                },
                candidate_ref: None,
            })
            .unwrap();
        assert_eq!(result["status"], "needsSelection");
        assert_eq!(result["candidates"].as_array().unwrap().len(), 2);
        let candidate_ref = result["candidates"][1]["candidateRef"]
            .as_str()
            .unwrap()
            .to_string();

        let cross_conversation = adapter
            .prepare(AgentSkillInstallationPrepareRequest {
                conversation_id: "conversation-2".to_string(),
                run_id: "run-x".to_string(),
                source: AgentSkillInstallationPrepareSource::Url {
                    url: source.to_string(),
                },
                candidate_ref: Some(candidate_ref.clone()),
            })
            .unwrap();
        assert_eq!(cross_conversation["status"], "invalid");
        assert_eq!(cross_conversation["error"]["code"], "candidateRefMismatch");

        let selected = adapter
            .prepare(AgentSkillInstallationPrepareRequest {
                conversation_id: "conversation-1".to_string(),
                run_id: "run-2".to_string(),
                source: AgentSkillInstallationPrepareSource::Url {
                    url: source.to_string(),
                },
                candidate_ref: Some(candidate_ref),
            })
            .unwrap();
        assert_eq!(selected["status"], "ready");
        assert_eq!(selected["name"], "fixture-1");
    }

    #[test]
    fn unsupported_or_non_skill_sources_return_model_safe_invalid_results() {
        let adapter = github_adapter(1);
        let invalid_url = adapter
            .prepare(AgentSkillInstallationPrepareRequest {
                conversation_id: "conversation-1".to_string(),
                run_id: "run-1".to_string(),
                source: AgentSkillInstallationPrepareSource::Url {
                    url: "http://github.com/example/skills".to_string(),
                },
                candidate_ref: None,
            })
            .unwrap();
        assert_eq!(invalid_url["status"], "invalid");
        assert!(invalid_url.get("resolutionId").is_none());

        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let source = fixture.path().join("not-a-skill");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("README.md"), "nothing here").unwrap();
        let workflow = Arc::new(SkillInstallationWorkflow::new(
            SkillInstallationService::new(store).unwrap(),
        ));
        let resolution = Arc::new(SkillSourceResolutionService::with_session_store(
            workflow.session_store(),
        ));
        let local_adapter = AgentSkillInstallationInspectionAdapter::new(resolution, workflow);
        let invalid_local = local_adapter
            .prepare(AgentSkillInstallationPrepareRequest {
                conversation_id: "conversation-1".to_string(),
                run_id: "run-1".to_string(),
                source: AgentSkillInstallationPrepareSource::LocalDirectory {
                    directory: source,
                    display_path: "not-a-skill".to_string(),
                },
                candidate_ref: None,
            })
            .unwrap();
        assert_eq!(invalid_local["status"], "invalid");
        assert!(invalid_local.get("preparationId").is_none());
    }
}
