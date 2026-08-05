use super::skill_installation_workflow_adapter::{commit_response, workflow_failure};
use super::skill_source_resolution_adapter::resolution_failure;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use mycopilot_core::skills::{
    GitHubReference, ResolvedSkillSource, SkillAcquisitionSource, SkillInstallationCommitRequest,
    SkillInstallationId, SkillInstallationPreparationRequest, SkillInstallationPreview,
    SkillInstallationSourceLocator, SkillInstallationWarningCode, SkillInstallationWorkflow,
    SkillPreparationId, SkillPreviewRevision, SkillSourceCandidateId, SkillSourceResolutionId,
    SkillSourceResolutionService,
};
use mycopilot_core::{
    AgentApprovalStatus, AgentError, AgentResult, AgentSkillInstallationCommitPreparationRequest,
    AgentSkillInstallationCommitPreparer, AgentSkillInstallationPrepareExecutor,
    AgentSkillInstallationPrepareRequest, AgentSkillInstallationPrepareSource,
    AgentSkillInstallationPreview, AgentSkillInstallationRequest,
    AgentSkillInstallationResourceSummary, AgentSkillInstallationWarning, AgentToolResult,
    AGENT_SKILL_INSTALLATION_SCHEMA_VERSION,
};
use mycopilot_protocol_rs::SkillInspectionPhaseDto;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
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
    pending_root: Option<PathBuf>,
}

impl std::fmt::Debug for AgentSkillInstallationInspectionAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentSkillInstallationInspectionAdapter")
            .finish_non_exhaustive()
    }
}

impl AgentSkillInstallationInspectionAdapter {
    #[cfg(test)]
    pub(crate) fn new(
        source_resolution: Arc<SkillSourceResolutionService>,
        workflow: Arc<SkillInstallationWorkflow>,
    ) -> Self {
        Self {
            source_resolution,
            workflow,
            refs: Mutex::new(InspectionRefs::default()),
            pending_root: None,
        }
    }

    pub(crate) fn with_pending_root(
        source_resolution: Arc<SkillSourceResolutionService>,
        workflow: Arc<SkillInstallationWorkflow>,
        pending_root: PathBuf,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(&pending_root)?;
        cleanup_expired_persisted_bindings(&pending_root, now_ms());
        Ok(Self {
            source_resolution,
            workflow,
            refs: Mutex::new(InspectionRefs::default()),
            pending_root: Some(pending_root),
        })
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
        let result = ready_projection(
            &install_ref,
            &preview,
            source_summary.clone(),
            resolved_revision.clone(),
        );
        let mut refs = self.refs.lock().unwrap_or_else(|error| error.into_inner());
        refs.prune(now_ms());
        if refs.install_refs.len() >= MAX_INSTALL_REFS {
            return Err(capacity_error());
        }
        let binding = InstallBinding {
            conversation_id: request.conversation_id.clone(),
            created_by_run_id: request.run_id.clone(),
            preparation_id: preview.preparation_id().clone(),
            preview_revision: preview.preview_revision().as_str().to_string(),
            preview: approval_preview(&preview, source_summary, resolved_revision),
            claimed_by: None,
            committed: false,
            expires_at_unix_ms: preview.expires_at_unix_ms(),
        };
        self.persist_binding(&install_ref, &binding)?;
        refs.install_refs.insert(install_ref.clone(), binding);
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

impl AgentSkillInstallationCommitPreparer for AgentSkillInstallationInspectionAdapter {
    fn prepare_commit_action(
        &self,
        request: AgentSkillInstallationCommitPreparationRequest,
    ) -> AgentResult<AgentSkillInstallationRequest> {
        let now = now_ms();
        self.ensure_binding_loaded(&request.install_ref)?;
        let mut refs = self.refs.lock().unwrap_or_else(|error| error.into_inner());
        refs.prune(now);
        let binding = refs
            .install_refs
            .get_mut(&request.install_ref)
            .ok_or_else(|| {
                commit_ref_error(
                    "installRefNotFound",
                    "The Skill installation reference is missing or expired.",
                    "prepareAgain",
                )
            })?;
        if binding.conversation_id != request.conversation_id {
            return Err(commit_ref_error(
                "installRefConversationMismatch",
                "The Skill installation reference does not belong to this conversation.",
                "prepareAgain",
            ));
        }
        if binding.created_by_run_id != request.run_id {
            return Err(commit_ref_error(
                "installRefRunMismatch",
                "The Skill installation reference does not belong to this run.",
                "prepareAgain",
            ));
        }
        if binding.committed {
            return Err(commit_ref_error(
                "alreadyCommitted",
                "This Skill installation transaction has already completed.",
                "startNextRun",
            ));
        }
        let claim = (request.run_id.clone(), request.action_id.clone());
        if binding
            .claimed_by
            .as_ref()
            .is_some_and(|existing| existing != &claim)
        {
            return Err(commit_ref_error(
                "installRefAlreadyClaimed",
                "The Skill installation reference is already bound to another approval action.",
                "prepareAgain",
            ));
        }
        binding.claimed_by = Some(claim);
        let action = AgentSkillInstallationRequest {
            schema_version: AGENT_SKILL_INSTALLATION_SCHEMA_VERSION,
            id: request.action_id,
            install_ref: request.install_ref,
            preview: binding.preview.clone(),
            approval_status: AgentApprovalStatus::Required,
            expires_at: binding.expires_at_unix_ms,
        };
        self.persist_binding(&action.install_ref, binding)?;
        Ok(action)
    }

    fn invalidate_commit_action(&self, action: &AgentSkillInstallationRequest) -> AgentResult<()> {
        self.cancel_action(action, None, None)
    }
}

impl AgentSkillInstallationInspectionAdapter {
    pub(crate) fn commit_approved(
        &self,
        action: &AgentSkillInstallationRequest,
        conversation_id: &str,
        run_id: &str,
    ) -> AgentToolResult {
        let binding = {
            if let Err(error) = self.ensure_binding_loaded(&action.install_ref) {
                return tool_result_from_agent_result(&action.id, Err(error));
            }
            let mut refs = self.refs.lock().unwrap_or_else(|error| error.into_inner());
            refs.prune(now_ms());
            refs.install_refs.get(&action.install_ref).cloned()
        };
        let result = (|| -> AgentResult<Value> {
            if action.schema_version != AGENT_SKILL_INSTALLATION_SCHEMA_VERSION
                || action.approval_status != AgentApprovalStatus::Approved
            {
                return Err(commit_ref_error(
                    "invalidApprovedSnapshot",
                    "The approved Skill installation snapshot is invalid.",
                    "prepareAgain",
                ));
            }
            let binding = binding.ok_or_else(|| {
                commit_ref_error(
                    "installRefNotFound",
                    "The frozen Skill package is missing or expired.",
                    "prepareAgain",
                )
            })?;
            if binding.conversation_id != conversation_id
                || binding.created_by_run_id != run_id
                || binding.claimed_by.as_ref() != Some(&(run_id.to_string(), action.id.clone()))
                || binding.preview != action.preview
                || binding.expires_at_unix_ms != action.expires_at
            {
                return Err(commit_ref_error(
                    "approvalIdentityMismatch",
                    "The approval does not match the frozen Skill installation transaction.",
                    "prepareAgain",
                ));
            }
            let preview_revision = SkillPreviewRevision::parse(binding.preview_revision.clone())
                .map_err(|_| invalid_host_state("The frozen preview revision is invalid."))?;
            let mut commit = SkillInstallationCommitRequest::new(
                binding.preparation_id.clone(),
                preview_revision,
            );
            for warning in &binding.preview.warnings {
                if !warning.requires_acknowledgement {
                    continue;
                }
                match warning.code.as_str() {
                    "containsScripts" => {
                        commit = commit.acknowledge(SkillInstallationWarningCode::ContainsScripts)
                    }
                    "resourcesNotExposed" => {
                        commit =
                            commit.acknowledge(SkillInstallationWarningCode::ResourcesNotExposed)
                    }
                    _ => {
                        return Err(commit_ref_error(
                            "unknownRequiredWarning",
                            "The frozen preview contains an unsupported required warning.",
                            "prepareAgain",
                        ))
                    }
                }
            }
            let committed = self.workflow.commit(&commit).map_err(|error| {
                let failure = workflow_failure(SkillInspectionPhaseDto::Commit, &error);
                agent_commit_failure(failure, false)
            })?;
            let response = commit_response(&committed)
                .map_err(|failure| agent_commit_failure(failure, true))?;
            let audit_value = serde_json::to_value(response)
                .map_err(|_| invalid_host_state("The Skill commit result could not be encoded."))?;
            let outcome = audit_value
                .get("outcome")
                .and_then(Value::as_str)
                .unwrap_or("installed");
            let status = if matches!(outcome, "alreadyInstalled" | "alreadyCurrent") {
                "alreadyInstalled"
            } else {
                "installed"
            };
            let mut refs = self.refs.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(current) = refs.install_refs.get_mut(&action.install_ref) {
                current.committed = true;
            }
            // Keep the frozen transaction until its natural expiry. If the process exits after
            // the Managed Skill Store commit but before the action result is durably settled,
            // startup recovery can replay this exact snapshot and receive AlreadyInstalled
            // instead of reacquiring mutable source bytes.
            // Authority-bearing workflow IDs remain in the shared installation service's audit
            // records. The Agent receives only the semantic outcome needed for its next reply.
            Ok(json!({
                "status": status,
                "outcome": outcome,
                "name": binding.preview.name,
                "availableFrom": "nextRun"
            }))
        })();
        tool_result_from_agent_result(&action.id, result)
    }

    pub(crate) fn reject_action(
        &self,
        action: &AgentSkillInstallationRequest,
        conversation_id: &str,
        run_id: &str,
    ) -> AgentResult<()> {
        self.cancel_action(action, Some(conversation_id), Some(run_id))
    }

    fn cancel_action(
        &self,
        action: &AgentSkillInstallationRequest,
        conversation_id: Option<&str>,
        run_id: Option<&str>,
    ) -> AgentResult<()> {
        let binding = {
            let mut refs = self.refs.lock().unwrap_or_else(|error| error.into_inner());
            refs.prune(now_ms());
            let binding = refs.install_refs.get(&action.install_ref).cloned();
            if let Some(binding) = binding.as_ref() {
                if conversation_id.is_some_and(|value| value != binding.conversation_id)
                    || run_id.is_some_and(|value| {
                        binding
                            .claimed_by
                            .as_ref()
                            .is_some_and(|claim| claim.0 != value)
                    })
                {
                    return Err(commit_ref_error(
                        "approvalIdentityMismatch",
                        "The rejected approval does not match this installation transaction.",
                        "prepareAgain",
                    ));
                }
            }
            refs.install_refs.remove(&action.install_ref);
            binding
        };
        if let Some(binding) = binding {
            let _ = self.workflow.cancel(&binding.preparation_id);
        }
        self.remove_persisted_binding(&action.install_ref);
        Ok(())
    }

    fn ensure_binding_loaded(&self, install_ref: &str) -> AgentResult<()> {
        if self
            .refs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .install_refs
            .contains_key(install_ref)
        {
            return Ok(());
        }
        let Some(path) = self.binding_path(install_ref) else {
            return Ok(());
        };
        let bytes = std::fs::read(&path).map_err(|_| {
            commit_ref_error(
                "installRefNotFound",
                "The Skill installation reference is missing or expired.",
                "prepareAgain",
            )
        })?;
        let document: PersistedInstallBinding = serde_json::from_slice(&bytes).map_err(|_| {
            commit_ref_error(
                "installSnapshotInvalid",
                "The persisted Skill installation snapshot is invalid.",
                "prepareAgain",
            )
        })?;
        if document.schema_version != 1 || document.install_ref != install_ref {
            return Err(commit_ref_error(
                "installSnapshotInvalid",
                "The persisted Skill installation snapshot identity is invalid.",
                "prepareAgain",
            ));
        }
        if document.expires_at_unix_ms <= now_ms() {
            let _ = std::fs::remove_file(&path);
            return Err(commit_ref_error(
                "installRefNotFound",
                "The Skill installation reference is missing or expired.",
                "prepareAgain",
            ));
        }
        let frozen = mycopilot_core::skills::SkillInstallationFrozenPreparation::from_bytes(
            BASE64.decode(document.frozen_preparation).map_err(|_| {
                commit_ref_error(
                    "installSnapshotInvalid",
                    "The persisted Skill package snapshot is invalid.",
                    "prepareAgain",
                )
            })?,
        )
        .map_err(|message| commit_ref_error("installSnapshotInvalid", &message, "prepareAgain"))?;
        let restored = self
            .workflow
            .restore_frozen_preparation(&frozen)
            .map_err(|message| {
                commit_ref_error("installSnapshotUnavailable", &message, "prepareAgain")
            })?;
        if restored.preview_revision().as_str() != document.preview_revision {
            return Err(commit_ref_error(
                "previewRevisionMismatch",
                "The persisted Skill preview changed and must be inspected again.",
                "prepareAgain",
            ));
        }
        let binding = InstallBinding {
            conversation_id: document.conversation_id,
            created_by_run_id: document.created_by_run_id,
            preparation_id: restored.preparation_id().clone(),
            preview_revision: document.preview_revision,
            preview: document.preview,
            claimed_by: document.claimed_by,
            committed: document.committed,
            expires_at_unix_ms: document.expires_at_unix_ms,
        };
        self.refs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .install_refs
            .insert(install_ref.to_string(), binding);
        Ok(())
    }

    fn persist_binding(&self, install_ref: &str, binding: &InstallBinding) -> AgentResult<()> {
        let Some(path) = self.binding_path(install_ref) else {
            return Ok(());
        };
        let frozen = self
            .workflow
            .export_frozen_preparation(&binding.preparation_id)
            .map_err(|message| {
                commit_ref_error("installSnapshotUnavailable", &message, "prepareAgain")
            })?;
        let document = PersistedInstallBinding {
            schema_version: 1,
            install_ref: install_ref.to_string(),
            conversation_id: binding.conversation_id.clone(),
            created_by_run_id: binding.created_by_run_id.clone(),
            preview_revision: binding.preview_revision.clone(),
            preview: binding.preview.clone(),
            claimed_by: binding.claimed_by.clone(),
            committed: binding.committed,
            expires_at_unix_ms: binding.expires_at_unix_ms,
            frozen_preparation: BASE64.encode(frozen.as_bytes()),
        };
        let bytes = serde_json::to_vec(&document).map_err(|_| {
            invalid_host_state("The frozen installation transaction could not be encoded.")
        })?;
        let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
        std::fs::write(&temporary, bytes).map_err(|_| {
            invalid_host_state("The frozen installation transaction could not be persisted.")
        })?;
        std::fs::rename(&temporary, &path).map_err(|_| {
            let _ = std::fs::remove_file(&temporary);
            invalid_host_state("The frozen installation transaction could not be published.")
        })?;
        Ok(())
    }

    fn remove_persisted_binding(&self, install_ref: &str) {
        if let Some(path) = self.binding_path(install_ref) {
            let _ = std::fs::remove_file(path);
        }
    }

    fn binding_path(&self, install_ref: &str) -> Option<PathBuf> {
        valid_install_ref_for_store(install_ref)
            .then(|| {
                self.pending_root
                    .as_ref()
                    .map(|root| root.join(format!("{install_ref}.json")))
            })
            .flatten()
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
    conversation_id: String,
    created_by_run_id: String,
    preparation_id: SkillPreparationId,
    preview_revision: String,
    preview: AgentSkillInstallationPreview,
    claimed_by: Option<(String, String)>,
    committed: bool,
    expires_at_unix_ms: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedInstallBinding {
    schema_version: u32,
    install_ref: String,
    conversation_id: String,
    created_by_run_id: String,
    preview_revision: String,
    preview: AgentSkillInstallationPreview,
    claimed_by: Option<(String, String)>,
    committed: bool,
    expires_at_unix_ms: u64,
    frozen_preparation: String,
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

fn approval_preview(
    preview: &SkillInstallationPreview,
    source_summary: Value,
    resolved_revision: String,
) -> AgentSkillInstallationPreview {
    let resources = preview.package().resources();
    let warnings = preview
        .warnings()
        .iter()
        .map(|warning| AgentSkillInstallationWarning {
            code: warning.code().stable_name().to_string(),
            message: warning.message().to_string(),
            requires_acknowledgement: warning.acknowledgement_required(),
        })
        .collect::<Vec<_>>();
    AgentSkillInstallationPreview {
        name: preview.package().name().to_string(),
        description: preview.package().description().to_string(),
        source_summary,
        resolved_revision,
        file_count: u64::try_from(resources.resource_count())
            .unwrap_or(u64::MAX)
            .saturating_add(1),
        total_bytes: preview.package().package_bytes(),
        resource_summary: AgentSkillInstallationResourceSummary {
            total: u64::try_from(resources.resource_count()).unwrap_or(u64::MAX),
            references: u64::try_from(resources.reference_count()).unwrap_or(u64::MAX),
            assets: u64::try_from(resources.asset_count()).unwrap_or(u64::MAX),
            scripts: u64::try_from(resources.script_count()).unwrap_or(u64::MAX),
            bytes: resources.resource_bytes(),
        },
        contains_scripts: resources.script_count() > 0,
        compatibility: if warnings.is_empty() {
            "compatible".to_string()
        } else {
            "compatibleWithWarnings".to_string()
        },
        warnings,
        operation: match preview.operation() {
            mycopilot_core::skills::SkillInstallationOperation::Install => "install",
            mycopilot_core::skills::SkillInstallationOperation::Update => "update",
            _ => "unknown",
        }
        .to_string(),
        impact: if preview.operation()
            == mycopilot_core::skills::SkillInstallationOperation::Install
        {
            "addManagedSkill"
        } else {
            "updateManagedSkill"
        }
        .to_string(),
    }
}

fn tool_result_from_agent_result(call_id: &str, result: AgentResult<Value>) -> AgentToolResult {
    match result {
        Ok(value) => AgentToolResult {
            exact_archive_file: None,
            call_id: call_id.to_string(),
            tool: "skills_commit_install".to_string(),
            ok: true,
            result: Some(value),
            error: None,
        },
        Err(error) => AgentToolResult {
            exact_archive_file: None,
            call_id: call_id.to_string(),
            tool: "skills_commit_install".to_string(),
            ok: false,
            result: error.details().cloned(),
            error: Some(error.to_string()),
        },
    }
}

fn commit_ref_error(code: &str, message: &str, recovery: &str) -> AgentError {
    AgentError::structured(
        format!("skill.installation.{code}"),
        message,
        json!({
            "type": "skillInstallation",
            "code": code,
            "recovery": recovery
        }),
    )
}

fn agent_commit_failure(
    failure: super::skill_installation_workflow_adapter::SkillInspectionFailure,
    committed_before_projection_failure: bool,
) -> AgentError {
    let message = failure.to_string();
    let uncertain = committed_before_projection_failure || failure.commit_may_have_succeeded();
    let code = if uncertain {
        "commitResultUncertain"
    } else {
        "commitFailed"
    };
    AgentError::structured(
        format!("skill.installation.{code}"),
        message,
        json!({
            "type": "skillInstallation",
            "code": code,
            "recovery": if uncertain { "refreshCatalogNextRun" } else { "prepareAgain" },
            "commitMayHaveSucceeded": uncertain
        }),
    )
}

fn invalid_resolution(error: mycopilot_core::skills::SkillSourceResolutionError) -> Value {
    let data = resolution_failure(error).into_data();
    let mut result = invalid_from_serializable(&*data);
    if let (Some(retry_after_ms), Some(output)) = (data.retry_after_ms, result.as_object_mut()) {
        output.insert("retryAfterMs".to_string(), json!(retry_after_ms));
    }
    result
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

fn valid_install_ref_for_store(value: &str) -> bool {
    value.strip_prefix("skill_install_").is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn cleanup_expired_persisted_bindings(root: &std::path::Path, now: u64) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if path.extension().and_then(|value| value.to_str()) != Some("json")
            || !valid_install_ref_for_store(stem)
        {
            continue;
        }
        let expired_or_invalid = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PersistedInstallBinding>(&bytes).ok())
            .is_none_or(|document| {
                document.schema_version != 1
                    || document.install_ref != stem
                    || document.expires_at_unix_ms <= now
            });
        if expired_or_invalid {
            let _ = std::fs::remove_file(path);
        }
    }
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

    fn persistent_local_adapter(
        store: &std::path::Path,
        pending: &std::path::Path,
    ) -> AgentSkillInstallationInspectionAdapter {
        let workflow = Arc::new(SkillInstallationWorkflow::new(
            SkillInstallationService::new(store).unwrap(),
        ));
        let resolution = Arc::new(SkillSourceResolutionService::with_session_store(
            workflow.session_store(),
        ));
        AgentSkillInstallationInspectionAdapter::with_pending_root(
            resolution,
            workflow,
            pending.to_path_buf(),
        )
        .unwrap()
    }

    fn prepare_local_action(
        adapter: &AgentSkillInstallationInspectionAdapter,
        source: &std::path::Path,
        action_id: &str,
    ) -> AgentSkillInstallationRequest {
        let prepared = adapter
            .prepare(AgentSkillInstallationPrepareRequest {
                conversation_id: "conversation-1".to_string(),
                run_id: "run-1".to_string(),
                source: AgentSkillInstallationPrepareSource::LocalDirectory {
                    directory: source.to_path_buf(),
                    display_path: "fixture-skill".to_string(),
                },
                candidate_ref: None,
            })
            .unwrap();
        adapter
            .prepare_commit_action(AgentSkillInstallationCommitPreparationRequest {
                conversation_id: "conversation-1".to_string(),
                run_id: "run-1".to_string(),
                action_id: action_id.to_string(),
                install_ref: prepared["installRef"].as_str().unwrap().to_string(),
            })
            .unwrap()
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
        assert_eq!(binding.conversation_id, "conversation-1");
        assert_eq!(binding.created_by_run_id, "run-1");
        assert!(!binding.preparation_id.as_str().is_empty());
        assert!(!binding.preview_revision.is_empty());
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

    #[test]
    fn model_safe_rate_limit_results_keep_only_the_actionable_retry_hint() {
        let result = invalid_resolution(
            SkillSourceResolutionError::resolve(
                SkillSourceResolutionErrorCode::RateLimited,
                SkillSourceResolutionRecovery::RetryLater,
                "provider body must not cross the boundary",
            )
            .with_retry_after(std::time::Duration::from_secs(19)),
        );

        assert_eq!(result["status"], "invalid");
        assert_eq!(result["error"]["code"], "rateLimited");
        assert_eq!(result["retryAfterMs"], 19_000);
        assert_eq!(result["recovery"], "retryLater");
        assert!(!result.to_string().contains("provider body"));
    }

    #[test]
    fn approved_commit_uses_the_frozen_snapshot_and_returns_only_agent_semantics() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let pending = fixture.path().join("pending");
        let source = fixture.path().join("source");
        std::fs::create_dir_all(source.join("scripts")).unwrap();
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: approved-skill\ndescription: Approval fixture.\n---\n\n# Fixture\n",
        )
        .unwrap();
        std::fs::write(source.join("scripts/run.py"), "print('fixture')\n").unwrap();
        let adapter = persistent_local_adapter(&store, &pending);
        let mut action = prepare_local_action(&adapter, &source, "action-1");

        assert!(action.preview.contains_scripts);
        assert!(action
            .preview
            .warnings
            .iter()
            .any(|warning| warning.requires_acknowledgement));
        assert!(SkillInstallationService::new(&store)
            .unwrap()
            .list_installed_skills()
            .unwrap()
            .records()
            .is_empty());

        action.approval_status = AgentApprovalStatus::Approved;
        let result = adapter.commit_approved(&action, "conversation-1", "run-1");
        assert!(result.ok, "{result:?}");
        let value = result.result.unwrap();
        assert_eq!(value["status"], "installed");
        assert_eq!(value["availableFrom"], "nextRun");
        for private in [
            "preparationId",
            "installationId",
            "previewRevision",
            "packageRevision",
        ] {
            assert!(value.get(private).is_none(), "leaked {private}");
        }
        assert_eq!(
            SkillInstallationService::new(&store)
                .unwrap()
                .list_installed_skills()
                .unwrap()
                .records()
                .len(),
            1
        );
    }

    #[test]
    fn pending_approval_survives_restart_and_replay_is_idempotent() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let pending = fixture.path().join("pending");
        let source = fixture.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: restart-skill\ndescription: Restart fixture.\n---\n\n# Fixture\n",
        )
        .unwrap();

        let first = persistent_local_adapter(&store, &pending);
        let mut action = prepare_local_action(&first, &source, "action-restart");
        action.approval_status = AgentApprovalStatus::Approved;
        drop(first);

        let after_restart = persistent_local_adapter(&store, &pending);
        let installed = after_restart.commit_approved(&action, "conversation-1", "run-1");
        assert!(installed.ok, "{installed:?}");
        drop(after_restart);

        let after_uncertain_ack = persistent_local_adapter(&store, &pending);
        let replayed = after_uncertain_ack.commit_approved(&action, "conversation-1", "run-1");
        assert!(replayed.ok, "{replayed:?}");
        assert_eq!(replayed.result.unwrap()["status"], "alreadyInstalled");
        assert_eq!(
            SkillInstallationService::new(&store)
                .unwrap()
                .list_installed_skills()
                .unwrap()
                .records()
                .len(),
            1
        );
    }

    #[test]
    fn rejection_cancels_the_frozen_transaction_without_mutating_the_store() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let pending = fixture.path().join("pending");
        let source = fixture.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: rejected-skill\ndescription: Rejection fixture.\n---\n\n# Fixture\n",
        )
        .unwrap();
        let adapter = persistent_local_adapter(&store, &pending);
        let action = prepare_local_action(&adapter, &source, "action-reject");

        adapter
            .reject_action(&action, "conversation-1", "run-1")
            .unwrap();
        assert!(SkillInstallationService::new(&store)
            .unwrap()
            .list_installed_skills()
            .unwrap()
            .records()
            .is_empty());
        assert!(!pending
            .join(format!("{}.json", action.install_ref))
            .exists());
    }

    #[test]
    fn claim_and_approved_preview_identity_are_immutable() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let pending = fixture.path().join("pending");
        let source = fixture.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: immutable-skill\ndescription: Immutable fixture.\n---\n\n# Fixture\n",
        )
        .unwrap();
        let adapter = persistent_local_adapter(&store, &pending);
        let action = prepare_local_action(&adapter, &source, "action-original");

        let cross_run_claim =
            adapter.prepare_commit_action(AgentSkillInstallationCommitPreparationRequest {
                conversation_id: "conversation-1".to_string(),
                run_id: "run-2".to_string(),
                action_id: "action-cross-run".to_string(),
                install_ref: action.install_ref.clone(),
            });
        assert_eq!(
            cross_run_claim.unwrap_err().code(),
            Some("skill.installation.installRefRunMismatch")
        );

        let second_claim =
            adapter.prepare_commit_action(AgentSkillInstallationCommitPreparationRequest {
                conversation_id: "conversation-1".to_string(),
                run_id: "run-1".to_string(),
                action_id: "action-replacement".to_string(),
                install_ref: action.install_ref.clone(),
            });
        assert_eq!(
            second_claim.unwrap_err().code(),
            Some("skill.installation.installRefAlreadyClaimed")
        );

        let mut tampered = action.clone();
        tampered.approval_status = AgentApprovalStatus::Approved;
        tampered.preview.name = "different-skill".to_string();
        let result = adapter.commit_approved(&tampered, "conversation-1", "run-1");
        assert!(!result.ok);
        assert_eq!(result.result.unwrap()["code"], "approvalIdentityMismatch");
        assert!(SkillInstallationService::new(&store)
            .unwrap()
            .list_installed_skills()
            .unwrap()
            .records()
            .is_empty());
    }
}
