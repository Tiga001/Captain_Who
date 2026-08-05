use super::*;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

const SNAPSHOT_SCHEMA_VERSION: u32 = 1;
const MAX_ENCODED_SNAPSHOT_BYTES: usize = MAX_SKILL_PACKAGE_BYTES * 2 + 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallationFrozenPreparation(Vec<u8>);

impl SkillInstallationFrozenPreparation {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, String> {
        if bytes.is_empty() || bytes.len() > MAX_ENCODED_SNAPSHOT_BYTES {
            return Err("Skill installation snapshot size is invalid.".to_string());
        }
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotDocument {
    schema_version: u32,
    preparation_id: String,
    installation_id: String,
    preview_revision: String,
    expires_at_unix_ms: u64,
    source: SourceDocument,
    acquisition: AcquisitionDocument,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum SourceDocument {
    LocalDirectory {
        path: String,
    },
    Adapter {
        provider: String,
        request: String,
    },
    ResolvedCandidate {
        resolution_id: String,
        candidate_id: String,
    },
    InstalledSource,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AcquisitionDocument {
    files: Vec<FileDocument>,
    origin_provider: String,
    origin_reference: String,
    authority: ProvenanceDocument,
    refresh: Option<ProvenanceDocument>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileDocument {
    path: String,
    bytes: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProvenanceDocument {
    provider: String,
    schema_version: u32,
    payload: String,
}

impl SkillInstallationWorkflow {
    /// Exports an exact, credential-free package snapshot for a pending approval.
    /// The blob is Host-only and must never be sent to the model or Renderer.
    pub fn export_frozen_preparation(
        &self,
        preparation_id: &SkillPreparationId,
    ) -> Result<SkillInstallationFrozenPreparation, String> {
        let now = self.sessions.now();
        let mut state = self
            .sessions
            .lock()
            .map_err(|_| "Skill installation session is unavailable.".to_string())?;
        state.prune_expired(now.monotonic);
        let Some(PreparationSlot::Ready {
            request,
            preview,
            acquisition,
            ..
        }) = state.preparations.get(preparation_id)
        else {
            return Err("Skill installation preparation is not ready or has expired.".to_string());
        };
        let installation_id = match request.intent() {
            SkillInstallationPreparationIntent::Install { installation_id } => installation_id,
            SkillInstallationPreparationIntent::Update { .. } => {
                return Err(
                    "Agent frozen installation snapshots currently support installs only."
                        .to_string(),
                )
            }
        };
        let document = SnapshotDocument {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            preparation_id: preparation_id.as_str().to_string(),
            installation_id: installation_id.as_str().to_string(),
            preview_revision: preview.preview_revision().as_str().to_string(),
            expires_at_unix_ms: preview.expires_at_unix_ms(),
            source: encode_source(request.source()),
            acquisition: encode_acquisition(acquisition),
        };
        let bytes = serde_json::to_vec(&document)
            .map_err(|_| "Skill installation snapshot could not be encoded.".to_string())?;
        SkillInstallationFrozenPreparation::from_bytes(bytes)
    }

    /// Restores a frozen package into the bounded workflow registry without reacquiring its
    /// original URL or local path. The original preview revision is recomputed and verified.
    pub fn restore_frozen_preparation(
        &self,
        frozen: &SkillInstallationFrozenPreparation,
    ) -> Result<SkillInstallationPreview, String> {
        let document: SnapshotDocument = serde_json::from_slice(frozen.as_bytes())
            .map_err(|_| "Skill installation snapshot is malformed.".to_string())?;
        if document.schema_version != SNAPSHOT_SCHEMA_VERSION {
            return Err("Skill installation snapshot schema is unsupported.".to_string());
        }
        let now = self.sessions.now();
        if document.expires_at_unix_ms <= now.unix_ms {
            return Err("Skill installation preparation has expired.".to_string());
        }
        let preparation_id = SkillPreparationId::parse(document.preparation_id)
            .map_err(|_| "Skill installation snapshot preparation ID is invalid.".to_string())?;
        let installation_id = SkillInstallationId::parse(document.installation_id)
            .map_err(|_| "Skill installation snapshot target is invalid.".to_string())?;
        let source = decode_source(document.source)?;
        let request = SkillInstallationPreparationRequest::install(
            preparation_id.clone(),
            installation_id,
            source,
        );
        let acquisition = decode_acquisition(document.acquisition)?;
        let action =
            PreparedAction::from_intent(request.intent()).map_err(|error| error.to_string())?;
        let preview = build_preview(
            &request,
            &action,
            &acquisition,
            None,
            document.expires_at_unix_ms,
        );
        if preview.preview_revision().as_str() != document.preview_revision {
            return Err("Skill installation snapshot preview revision does not match.".to_string());
        }
        let snapshot_bytes = acquisition_snapshot_bytes(&acquisition);
        let remaining = Duration::from_millis(document.expires_at_unix_ms - now.unix_ms);
        let mut state = self
            .sessions
            .lock()
            .map_err(|_| "Skill installation session is unavailable.".to_string())?;
        state.prune_expired(now.monotonic);
        match state.preparations.get(&preparation_id) {
            Some(PreparationSlot::Ready {
                preview: existing, ..
            })
            | Some(PreparationSlot::Committed {
                preview: existing, ..
            }) => {
                if existing.preview_revision() == preview.preview_revision() {
                    return Ok(existing.clone());
                }
                return Err("A different Skill preparation already uses this identity.".to_string());
            }
            Some(_) => return Err("The Skill preparation is currently busy.".to_string()),
            None => {}
        }
        if state
            .reserved_snapshot_bytes()
            .saturating_add(snapshot_bytes)
            > self.sessions.config().max_snapshot_bytes()
        {
            return Err("Skill installation snapshot capacity is exceeded.".to_string());
        }
        state.preparations.insert(
            preparation_id,
            PreparationSlot::Ready {
                request,
                preview: preview.clone(),
                acquisition,
                snapshot_bytes,
                expires_at: now.monotonic.saturating_add(remaining),
            },
        );
        self.sessions.notify_all();
        Ok(preview)
    }
}

fn encode_source(source: &SkillAcquisitionSource) -> SourceDocument {
    match source {
        SkillAcquisitionSource::LocalDirectory { directory } => SourceDocument::LocalDirectory {
            path: directory.to_string_lossy().into_owned(),
        },
        SkillAcquisitionSource::Adapter { provider, request } => SourceDocument::Adapter {
            provider: provider.as_str().to_string(),
            request: BASE64.encode(request),
        },
        SkillAcquisitionSource::ResolvedCandidate {
            resolution_id,
            candidate_id,
        } => SourceDocument::ResolvedCandidate {
            resolution_id: resolution_id.as_str().to_string(),
            candidate_id: candidate_id.as_str().to_string(),
        },
        SkillAcquisitionSource::InstalledSource => SourceDocument::InstalledSource,
    }
}

fn decode_source(source: SourceDocument) -> Result<SkillAcquisitionSource, String> {
    match source {
        SourceDocument::LocalDirectory { path } => {
            Ok(SkillAcquisitionSource::local_directory(PathBuf::from(path)))
        }
        SourceDocument::Adapter { provider, request } => SkillAcquisitionSource::adapter(
            SkillAcquisitionProvider::parse(provider)
                .map_err(|_| "Frozen acquisition provider is invalid.".to_string())?,
            BASE64
                .decode(request)
                .map_err(|_| "Frozen acquisition request is invalid.".to_string())?,
        )
        .map_err(|error| error.to_string()),
        SourceDocument::ResolvedCandidate {
            resolution_id,
            candidate_id,
        } => Ok(SkillAcquisitionSource::resolved_candidate(
            SkillSourceResolutionId::parse(resolution_id)
                .map_err(|_| "Frozen resolution ID is invalid.".to_string())?,
            SkillSourceCandidateId::parse(candidate_id)
                .map_err(|_| "Frozen candidate ID is invalid.".to_string())?,
        )),
        SourceDocument::InstalledSource => Ok(SkillAcquisitionSource::installed_source()),
    }
}

fn encode_acquisition(acquisition: &PreparedSkillAcquisition) -> AcquisitionDocument {
    let package = acquisition.package();
    let mut files = vec![FileDocument {
        path: "SKILL.md".to_string(),
        bytes: BASE64.encode(package.source_bytes()),
    }];
    files.extend(package.resources().iter().map(|resource| FileDocument {
        path: resource.descriptor().path().to_string(),
        bytes: BASE64.encode(resource.bytes()),
    }));
    let provenance = acquisition.provenance();
    AcquisitionDocument {
        files,
        origin_provider: package.origin().provider().to_string(),
        origin_reference: package.origin().reference().to_string(),
        authority: ProvenanceDocument {
            provider: provenance.authority().provider().to_string(),
            schema_version: provenance.authority().schema_version(),
            payload: provenance.authority().payload().to_string(),
        },
        refresh: provenance.refresh().map(|refresh| ProvenanceDocument {
            provider: refresh.provider().to_string(),
            schema_version: refresh.schema_version(),
            payload: refresh.payload().to_string(),
        }),
    }
}

fn decode_acquisition(document: AcquisitionDocument) -> Result<PreparedSkillAcquisition, String> {
    let files = document
        .files
        .into_iter()
        .map(|file| {
            BASE64
                .decode(file.bytes)
                .map(|bytes| (file.path, bytes))
                .map_err(|_| "Frozen Skill file bytes are invalid.".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let origin = super::super::origin::SkillPackageOrigin::new(
        document.origin_provider,
        document.origin_reference,
    )
    .map_err(|error| error.to_string())?;
    let package =
        PreparedSkillPackage::from_files(files, origin).map_err(|error| error.to_string())?;
    let authority = SkillInstallationAuthority::new(
        document.authority.provider,
        document.authority.schema_version,
        document.authority.payload,
    )
    .map_err(|error| error.to_string())?;
    let refresh = document
        .refresh
        .map(|refresh| {
            SkillInstallationRefresh::new(refresh.provider, refresh.schema_version, refresh.payload)
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    let acquisition = PreparedSkillAcquisition::new(
        package,
        SkillInstallationProvenance::new(authority, refresh),
    );
    if !acquisition.is_owned_by(acquisition.package().origin().provider()) {
        return Err("Frozen Skill acquisition authority does not match its package.".to_string());
    }
    Ok(acquisition)
}
