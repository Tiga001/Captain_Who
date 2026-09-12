//! Provider-neutral, backend-owned snapshots of the world visible to an agent.
//!
//! World state is deliberately separate from conversation history. A full snapshot establishes an
//! exact state at the start of an epoch; ordered diffs then describe section-level changes. The
//! authoritative state and its model projection are separate so host-only data can never leak merely
//! because a renderer serializes the domain object.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    str::FromStr,
};

mod workspace_projection;
pub use workspace_projection::{
    WorkspaceFolderModelChange, WorkspaceFolderModelProjection, WorkspaceFolderUpdateReason,
};

pub const WORLD_STATE_SCHEMA_VERSION: u32 = 1;
pub const WORLD_STATE_REVISION_PREFIX: &str = "world-state-sha256-v1:";

const MODEL_RECORD_TAG: &str = "backend_world_state_record";
const MAX_SECTION_ID_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldStateLifetime {
    Conversation,
    Run,
    Request,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldStateVisibility {
    ModelVisible,
    HostOnly,
}

/// Stable identifiers for sections owned by the backend.
///
/// `Extension` keeps the journal forward-compatible without weakening validation: extension IDs
/// must use the same stable lowercase dotted identifier format as built-in IDs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorldStateSectionId {
    EffectivePermissions,
    WorkspaceBinding,
    InteractionProfile,
    EffectiveTools,
    SkillActivation,
    AttachmentLibrarySummary,
    ModelSelection,
    ModelCapabilities,
    Environment,
    Extension(String),
}

impl WorldStateSectionId {
    pub const EFFECTIVE_PERMISSIONS: &'static str = "permissions.effective";
    pub const WORKSPACE_BINDING: &'static str = "workspace.binding";
    pub const INTERACTION_PROFILE: &'static str = "interaction.profile";
    pub const EFFECTIVE_TOOLS: &'static str = "tools.effective";
    pub const SKILL_ACTIVATION: &'static str = "skills.activation";
    pub const ATTACHMENT_LIBRARY_SUMMARY: &'static str = "attachments.library_summary";
    pub const MODEL_SELECTION: &'static str = "model.selection";
    pub const MODEL_CAPABILITIES: &'static str = "model.capabilities";
    pub const ENVIRONMENT: &'static str = "environment";

    pub fn as_str(&self) -> &str {
        match self {
            Self::EffectivePermissions => Self::EFFECTIVE_PERMISSIONS,
            Self::WorkspaceBinding => Self::WORKSPACE_BINDING,
            Self::InteractionProfile => Self::INTERACTION_PROFILE,
            Self::EffectiveTools => Self::EFFECTIVE_TOOLS,
            Self::SkillActivation => Self::SKILL_ACTIVATION,
            Self::AttachmentLibrarySummary => Self::ATTACHMENT_LIBRARY_SUMMARY,
            Self::ModelSelection => Self::MODEL_SELECTION,
            Self::ModelCapabilities => Self::MODEL_CAPABILITIES,
            Self::Environment => Self::ENVIRONMENT,
            Self::Extension(value) => value,
        }
    }

    pub fn extension(value: impl Into<String>) -> Result<Self, WorldStateError> {
        let value = value.into();
        validate_section_id(&value)?;
        Ok(Self::from_known_or_extension(value))
    }

    fn from_known_or_extension(value: String) -> Self {
        match value.as_str() {
            Self::EFFECTIVE_PERMISSIONS => Self::EffectivePermissions,
            Self::WORKSPACE_BINDING => Self::WorkspaceBinding,
            Self::INTERACTION_PROFILE => Self::InteractionProfile,
            Self::EFFECTIVE_TOOLS => Self::EffectiveTools,
            Self::SKILL_ACTIVATION => Self::SkillActivation,
            Self::ATTACHMENT_LIBRARY_SUMMARY => Self::AttachmentLibrarySummary,
            Self::MODEL_SELECTION => Self::ModelSelection,
            Self::MODEL_CAPABILITIES => Self::ModelCapabilities,
            Self::ENVIRONMENT => Self::Environment,
            _ => Self::Extension(value),
        }
    }
}

impl fmt::Display for WorldStateSectionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for WorldStateSectionId {
    type Err = WorldStateError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        validate_section_id(value)?;
        Ok(Self::from_known_or_extension(value.to_string()))
    }
}

impl Ord for WorldStateSectionId {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialOrd for WorldStateSectionId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Serialize for WorldStateSectionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for WorldStateSectionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

/// A section keeps authoritative host state distinct from the value explicitly approved for model
/// context. A model-visible section must provide a projection; a host-only section must not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorldStateSectionEnvelope {
    pub schema_version: u32,
    pub id: WorldStateSectionId,
    pub lifetime: WorldStateLifetime,
    pub visibility: WorldStateVisibility,
    pub revision: String,
    pub state: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_projection: Option<Value>,
}

impl WorldStateSectionEnvelope {
    pub fn model_visible(
        id: WorldStateSectionId,
        lifetime: WorldStateLifetime,
        state: Value,
        model_projection: Value,
    ) -> Result<Self, WorldStateError> {
        Self::new(
            id,
            lifetime,
            WorldStateVisibility::ModelVisible,
            state,
            Some(model_projection),
        )
    }

    pub fn host_only(
        id: WorldStateSectionId,
        lifetime: WorldStateLifetime,
        state: Value,
    ) -> Result<Self, WorldStateError> {
        Self::new(id, lifetime, WorldStateVisibility::HostOnly, state, None)
    }

    pub fn new(
        id: WorldStateSectionId,
        lifetime: WorldStateLifetime,
        visibility: WorldStateVisibility,
        state: Value,
        model_projection: Option<Value>,
    ) -> Result<Self, WorldStateError> {
        let mut section = Self {
            schema_version: WORLD_STATE_SCHEMA_VERSION,
            id,
            lifetime,
            visibility,
            revision: String::new(),
            state,
            model_projection,
        };
        section.validate_projection_contract()?;
        section.revision = section.computed_revision();
        section.validate()?;
        Ok(section)
    }

    pub fn validate(&self) -> Result<(), WorldStateError> {
        validate_schema_version(self.schema_version)?;
        validate_section_id(self.id.as_str())?;
        self.validate_projection_contract()?;
        validate_revision("section revision", &self.revision)?;
        let expected = self.computed_revision();
        if self.revision != expected {
            return Err(WorldStateError::SectionRevisionMismatch {
                section_id: self.id.clone(),
                expected,
                actual: self.revision.clone(),
            });
        }
        Ok(())
    }

    pub fn computed_revision(&self) -> String {
        revision_for_value(&self.revision_material())
    }

    fn validate_projection_contract(&self) -> Result<(), WorldStateError> {
        match (self.visibility, self.model_projection.as_ref()) {
            (WorldStateVisibility::ModelVisible, None) => {
                Err(WorldStateError::ModelProjectionRequired(self.id.clone()))
            }
            (WorldStateVisibility::HostOnly, Some(_)) => Err(
                WorldStateError::HostOnlyProjectionForbidden(self.id.clone()),
            ),
            _ => Ok(()),
        }
    }

    fn revision_material(&self) -> Value {
        let mut material = Map::new();
        material.insert(
            "id".to_string(),
            Value::String(self.id.as_str().to_string()),
        );
        material.insert(
            "lifetime".to_string(),
            serde_json::to_value(self.lifetime).expect("world state lifetime is serializable"),
        );
        material.insert(
            "modelProjection".to_string(),
            self.model_projection.clone().unwrap_or(Value::Null),
        );
        material.insert(
            "schemaVersion".to_string(),
            Value::from(self.schema_version),
        );
        material.insert("state".to_string(), self.state.clone());
        material.insert(
            "visibility".to_string(),
            serde_json::to_value(self.visibility).expect("world state visibility is serializable"),
        );
        Value::Object(material)
    }
}

/// Builds the canonical model-visible projection of the permissions currently enforced by the
/// trusted Host. Runtime execution must continue to use the typed permission value directly; this
/// section is only the provider-neutral state presented to the model.
pub fn effective_permissions_section(
    permissions: crate::protocol::AgentPermissions,
    lifetime: WorldStateLifetime,
) -> Result<WorldStateSectionEnvelope, WorldStateError> {
    let state = serde_json::json!({
        "read": match permissions.read {
            crate::protocol::AgentReadPermission::WorkspaceOnly => "workspace_only",
            crate::protocol::AgentReadPermission::All => "all",
        },
        "write": match permissions.write {
            crate::protocol::AgentWritePermission::Denied => "denied",
            crate::protocol::AgentWritePermission::WorkspaceOnly => "workspace_only",
            crate::protocol::AgentWritePermission::All => "all",
        },
        "command": match permissions.command {
            crate::protocol::AgentCommandPermission::RequireApproval => "require_approval",
            crate::protocol::AgentCommandPermission::AutoApprove => "auto_approve",
        },
        "commandSafety": match permissions.command_safety {
            crate::protocol::AgentCommandSafetyPolicy::Guarded => "guarded",
            crate::protocol::AgentCommandSafetyPolicy::FullAccess => "full_access",
        },
        "patch": match permissions.patch {
            crate::protocol::AgentPatchPermission::RequireApproval => "require_approval",
            crate::protocol::AgentPatchPermission::AutoApprove => "auto_approve",
        },
        "builtinExecution": match permissions.builtin_execution {
            crate::protocol::AgentBuiltinExecutionPermission::RequireApproval => "require_approval",
            crate::protocol::AgentBuiltinExecutionPermission::AutoApprove => "auto_approve",
        },
    });
    WorldStateSectionEnvelope::model_visible(
        WorldStateSectionId::EffectivePermissions,
        lifetime,
        state.clone(),
        state,
    )
}

/// Builds the canonical workspace binding. The authoritative root remains Host-visible while the
/// model receives only the selected scope and the path convention it should use.
pub fn workspace_binding_section(
    workspace: Option<&crate::protocol::AgentWorkspaceContext>,
    lifetime: WorldStateLifetime,
) -> Result<WorldStateSectionEnvelope, WorldStateError> {
    let available = workspace.is_some_and(|workspace| {
        if workspace.folders.is_empty() {
            workspace
                .root_path
                .as_deref()
                .is_some_and(|root| !root.trim().is_empty())
        } else {
            workspace.folders.iter().any(|folder| {
                folder.role == crate::storage::models::ProjectFolderRole::Primary
                    && folder.canonical_path.is_some()
            })
        }
    });
    let state = serde_json::json!({
        "available": available,
        "projectId": workspace.and_then(|workspace| workspace.project_id.as_deref()),
        "displayName": workspace.and_then(|workspace| workspace.display_name.as_deref()),
        "rootPath": workspace.and_then(|workspace| workspace.root_path.as_deref()),
        "folders": workspace.map(|workspace| &workspace.folders),
    });
    let mut folders = workspace
        .map(|workspace| {
            workspace
                .folders
                .iter()
                .map(WorkspaceFolderModelProjection::from)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // UI ordering is not a workspace capability change. A canonical alias order also keeps
    // equivalent full baselines stable across project edits and context rebases.
    folders.sort_by(|left, right| left.alias.cmp(&right.alias));
    let projection = serde_json::json!({
        "available": available,
        "displayName": workspace.and_then(|workspace| workspace.display_name.as_deref()),
        "pathConvention": if available { "workspace_relative" } else { "no_workspace" },
        "folders": folders,
        "defaultScope": if available { "primary_only" } else { "no_workspace" },
    });
    WorldStateSectionEnvelope::model_visible(
        WorldStateSectionId::WorkspaceBinding,
        lifetime,
        state,
        projection,
    )
}

/// Builds the canonical interaction profile independently of its conversation/run placement.
pub fn interaction_profile_section(
    preferences: Option<&crate::protocol::AgentPromptPreferences>,
    lifetime: WorldStateLifetime,
) -> Result<WorldStateSectionEnvelope, WorldStateError> {
    let profile = preferences
        .map(|value| value.context_profile)
        .unwrap_or_default();
    let state = serde_json::json!({
        "contextProfile": profile,
        "contextProfileDescription": match profile {
            crate::protocol::AgentContextProfile::Full => "Full base instructions and tools; extensions follow their existing settings.",
            crate::protocol::AgentContextProfile::Minimal => "Concise base instructions and tool descriptions with fewer base tools; extensions follow their existing settings.",
        },
        "workMode": match preferences.and_then(|value| value.work_mode) {
            Some(crate::protocol::AgentPromptWorkMode::General) => "general",
            Some(crate::protocol::AgentPromptWorkMode::Coding) | None => "coding",
        },
        "tone": match preferences.and_then(|value| value.tone) {
            Some(crate::protocol::AgentPromptTone::Friendly) => "friendly",
            Some(crate::protocol::AgentPromptTone::Pragmatic) | None => "pragmatic",
        },
        "detailLevel": match preferences.and_then(|value| value.detail_level) {
            Some(crate::protocol::AgentPromptDetailLevel::Low) => "low",
            Some(crate::protocol::AgentPromptDetailLevel::High) => "high",
            Some(crate::protocol::AgentPromptDetailLevel::Medium) | None => "medium",
        },
    });
    WorldStateSectionEnvelope::model_visible(
        WorldStateSectionId::InteractionProfile,
        lifetime,
        state.clone(),
        state,
    )
}

/// Builds the model-visible identity and image-input capability selected by the Host for the
/// current conversation. These values describe configured routing rather than claiming a
/// provider-verified model identity; execution continues to use the separate Host-only capability
/// section.
pub fn model_selection_section(
    configured_model_id: &str,
    capabilities: crate::protocol::ModelCapabilities,
    lifetime: WorldStateLifetime,
) -> Result<WorldStateSectionEnvelope, WorldStateError> {
    let state = serde_json::json!({
        "configuredModelId": configured_model_id,
        "capabilities": {
            "imageInput": capabilities.image_input,
        }
    });
    WorldStateSectionEnvelope::model_visible(
        WorldStateSectionId::ModelSelection,
        lifetime,
        state.clone(),
        state,
    )
}

/// Process-observed environment facts shared by actual Host requests and read-only previews.
/// Operational capability and grant state belongs to its owner section, not this environment.
pub fn environment_section(
    lifetime: WorldStateLifetime,
) -> Result<WorldStateSectionEnvelope, WorldStateError> {
    let value = environment_projection();
    WorldStateSectionEnvelope::model_visible(
        WorldStateSectionId::Environment,
        lifetime,
        value.clone(),
        value,
    )
}

fn environment_projection() -> serde_json::Value {
    let shell_name = std::env::var("SHELL")
        .ok()
        .or_else(|| std::env::var("COMSPEC").ok())
        .and_then(|shell| {
            std::path::Path::new(&shell)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        });
    let timezone = std::env::var("TZ")
        .ok()
        .filter(|value| !value.trim().is_empty());
    serde_json::json!({
        "os": {
            "family": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "shell": {
            "name": shell_name,
        },
        "timezone": {
            "name": timezone,
            "source": if timezone.is_some() { "TZ" } else { "system" },
        },
        "network": {
            "publicWeb": "tool_gated",
            "note": "Use registered web tools when available; do not infer arbitrary network access.",
        }
    })
}

/// Complete model capabilities remain Host-only execution authority even though a narrow derived
/// projection is exposed through `model.selection` in the same versioned World State ledger.
pub fn model_capabilities_section(
    capabilities: crate::protocol::ModelCapabilities,
    lifetime: WorldStateLifetime,
) -> Result<WorldStateSectionEnvelope, WorldStateError> {
    WorldStateSectionEnvelope::host_only(
        WorldStateSectionId::ModelCapabilities,
        lifetime,
        serde_json::json!({
            "imageInput": capabilities.image_input,
        }),
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorldStateSnapshot {
    pub schema_version: u32,
    pub epoch_id: String,
    pub sequence: u64,
    pub revision: String,
    pub sections: Vec<WorldStateSectionEnvelope>,
}

impl WorldStateSnapshot {
    pub fn new(
        epoch_id: impl Into<String>,
        sequence: u64,
        mut sections: Vec<WorldStateSectionEnvelope>,
    ) -> Result<Self, WorldStateError> {
        sections.sort_by(|left, right| left.id.cmp(&right.id));
        ensure_unique_sections(&sections)?;
        let mut snapshot = Self {
            schema_version: WORLD_STATE_SCHEMA_VERSION,
            epoch_id: epoch_id.into(),
            sequence,
            revision: String::new(),
            sections,
        };
        snapshot.revision = snapshot.computed_revision();
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> Result<(), WorldStateError> {
        validate_schema_version(self.schema_version)?;
        validate_epoch_id(&self.epoch_id)?;
        ensure_canonical_sections(&self.sections)?;
        for section in &self.sections {
            section.validate()?;
        }
        validate_revision("snapshot revision", &self.revision)?;
        let expected = self.computed_revision();
        if self.revision != expected {
            return Err(WorldStateError::SnapshotRevisionMismatch {
                expected,
                actual: self.revision.clone(),
            });
        }
        Ok(())
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn canonical_json(&self) -> String {
        canonical_json_string(
            &serde_json::to_value(self).expect("world state snapshot is serializable"),
        )
    }

    pub fn section(&self, id: &WorldStateSectionId) -> Option<&WorldStateSectionEnvelope> {
        self.sections
            .binary_search_by(|candidate| candidate.id.cmp(id))
            .ok()
            .map(|index| &self.sections[index])
    }

    pub fn computed_revision(&self) -> String {
        let sections = self
            .sections
            .iter()
            .map(WorldStateSectionEnvelope::revision_material)
            .collect::<Vec<_>>();
        let mut material = Map::new();
        material.insert(
            "schemaVersion".to_string(),
            Value::from(self.schema_version),
        );
        material.insert("sections".to_string(), Value::Array(sections));
        revision_for_value(&Value::Object(material))
    }

    pub fn model_projection_revision(
        &self,
        expected_lifetime: WorldStateLifetime,
    ) -> Result<String, WorldStateError> {
        Ok(revision_for_value(
            &self.model_projection(expected_lifetime)?.canonical_value(),
        ))
    }

    /// Produces the sanitized model view for exactly one independent lifetime ledger.
    ///
    /// Callers must state which ledger they are rendering. An empty snapshot is valid for any
    /// expected lifetime; a non-empty snapshot must contain only sections from that lifetime.
    pub fn model_projection(
        &self,
        expected_lifetime: WorldStateLifetime,
    ) -> Result<WorldStateModelRecord, WorldStateError> {
        self.validate()?;
        validate_snapshot_lifetime(self, expected_lifetime)?;
        Ok(WorldStateModelRecord::Full {
            schema_version: self.schema_version,
            lifetime: expected_lifetime,
            sections: self
                .sections
                .iter()
                .filter_map(|section| {
                    section
                        .model_projection
                        .as_ref()
                        .map(|value| WorldStateModelSection {
                            id: section.id.clone(),
                            value: value.clone(),
                        })
                })
                .collect(),
        })
    }

    /// Starts a new cache/compaction epoch without changing the exact state revision.
    pub fn rebase(
        &self,
        new_epoch_id: impl Into<String>,
    ) -> Result<WorldStateSnapshot, WorldStateError> {
        Self::new(new_epoch_id, 0, self.sections.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorldStateSectionPrecondition {
    pub revision: String,
    pub visibility: WorldStateVisibility,
}

impl WorldStateSectionPrecondition {
    fn from_section(section: &WorldStateSectionEnvelope) -> Self {
        Self {
            revision: section.revision.clone(),
            visibility: section.visibility,
        }
    }

    fn validate(&self) -> Result<(), WorldStateError> {
        validate_revision("section precondition revision", &self.revision)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorldStateSectionTombstone {
    pub section_id: WorldStateSectionId,
    pub removed_revision: String,
    pub removed_visibility: WorldStateVisibility,
}

impl WorldStateSectionTombstone {
    fn from_section(section: &WorldStateSectionEnvelope) -> Self {
        Self {
            section_id: section.id.clone(),
            removed_revision: section.revision.clone(),
            removed_visibility: section.visibility,
        }
    }

    fn validate(&self) -> Result<(), WorldStateError> {
        validate_section_id(self.section_id.as_str())?;
        validate_revision("removed section revision", &self.removed_revision)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "op",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum WorldStateOperation {
    Add {
        section: WorldStateSectionEnvelope,
    },
    Replace {
        previous: WorldStateSectionPrecondition,
        section: WorldStateSectionEnvelope,
    },
    Remove {
        tombstone: WorldStateSectionTombstone,
    },
}

impl WorldStateOperation {
    pub fn section_id(&self) -> &WorldStateSectionId {
        match self {
            Self::Add { section } | Self::Replace { section, .. } => &section.id,
            Self::Remove { tombstone } => &tombstone.section_id,
        }
    }

    fn validate(&self) -> Result<(), WorldStateError> {
        match self {
            Self::Add { section } => section.validate(),
            Self::Replace { previous, section } => {
                previous.validate()?;
                section.validate()
            }
            Self::Remove { tombstone } => tombstone.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorldStateDiff {
    pub schema_version: u32,
    pub epoch_id: String,
    pub sequence: u64,
    pub base_revision: String,
    pub result_revision: String,
    pub operations: Vec<WorldStateOperation>,
}

impl WorldStateDiff {
    pub fn from_operations(
        base: &WorldStateSnapshot,
        sequence: u64,
        mut operations: Vec<WorldStateOperation>,
    ) -> Result<Self, WorldStateError> {
        base.validate()?;
        let expected_sequence = next_sequence(base.sequence)?;
        if sequence != expected_sequence {
            return Err(WorldStateError::SequenceMismatch {
                expected: expected_sequence,
                actual: sequence,
            });
        }
        operations.sort_by(|left, right| left.section_id().cmp(right.section_id()));
        ensure_canonical_operations(&operations)?;
        let mut diff = Self {
            schema_version: WORLD_STATE_SCHEMA_VERSION,
            epoch_id: base.epoch_id.clone(),
            sequence,
            base_revision: base.revision.clone(),
            result_revision: String::new(),
            operations,
        };
        let result = apply_diff_unchecked_result(base, &diff)?;
        diff.result_revision = result.revision;
        diff.validate()?;
        Ok(diff)
    }

    pub fn between(
        base: &WorldStateSnapshot,
        target: &WorldStateSnapshot,
    ) -> Result<Self, WorldStateError> {
        base.validate()?;
        target.validate()?;
        if base.epoch_id != target.epoch_id {
            return Err(WorldStateError::EpochMismatch {
                expected: base.epoch_id.clone(),
                actual: target.epoch_id.clone(),
            });
        }
        let expected_sequence = next_sequence(base.sequence)?;
        if target.sequence != expected_sequence {
            return Err(WorldStateError::SequenceMismatch {
                expected: expected_sequence,
                actual: target.sequence,
            });
        }

        let base_sections = section_map(&base.sections);
        let target_sections = section_map(&target.sections);
        let section_ids = base_sections
            .keys()
            .chain(target_sections.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut operations = Vec::new();
        for section_id in section_ids {
            match (
                base_sections.get(&section_id),
                target_sections.get(&section_id),
            ) {
                (None, Some(section)) => operations.push(WorldStateOperation::Add {
                    section: (*section).clone(),
                }),
                (Some(section), None) => operations.push(WorldStateOperation::Remove {
                    tombstone: WorldStateSectionTombstone::from_section(section),
                }),
                (Some(previous), Some(section)) if previous.revision != section.revision => {
                    operations.push(WorldStateOperation::Replace {
                        previous: WorldStateSectionPrecondition::from_section(previous),
                        section: (*section).clone(),
                    });
                }
                _ => {}
            }
        }

        let diff = Self::from_operations(base, target.sequence, operations)?;
        if diff.result_revision != target.revision {
            return Err(WorldStateError::ResultRevisionMismatch {
                expected: target.revision.clone(),
                actual: diff.result_revision,
            });
        }
        Ok(diff)
    }

    pub fn validate(&self) -> Result<(), WorldStateError> {
        validate_schema_version(self.schema_version)?;
        validate_epoch_id(&self.epoch_id)?;
        validate_revision("diff base revision", &self.base_revision)?;
        validate_revision("diff result revision", &self.result_revision)?;
        ensure_canonical_operations(&self.operations)?;
        for operation in &self.operations {
            operation.validate()?;
        }
        Ok(())
    }

    pub fn base_revision(&self) -> &str {
        &self.base_revision
    }

    pub fn revision(&self) -> &str {
        &self.result_revision
    }

    pub fn result_revision(&self) -> &str {
        &self.result_revision
    }

    pub fn canonical_json(&self) -> String {
        canonical_json_string(
            &serde_json::to_value(self).expect("world state diff is serializable"),
        )
    }

    /// Produces only the model-visible change between a validated base and this diff.
    ///
    /// Returning `None` means authoritative host state changed while the sanitized model view did
    /// not. Epoch IDs, sequence numbers, hashes, timestamps and authoritative host values are never
    /// serialized into this record.
    pub fn model_projection_against(
        &self,
        base: &WorldStateSnapshot,
        expected_lifetime: WorldStateLifetime,
    ) -> Result<Option<WorldStateModelRecord>, WorldStateError> {
        validate_snapshot_lifetime(base, expected_lifetime)?;
        let result = WorldStateReducer::fold(base.clone(), std::slice::from_ref(self))?;
        validate_snapshot_lifetime(&result, expected_lifetime)?;
        let before = model_section_map(base);
        let after = model_section_map(&result);
        let section_ids = before
            .keys()
            .chain(after.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut changes = Vec::new();
        for section_id in section_ids {
            if section_id == WorldStateSectionId::WorkspaceBinding {
                if let Some(workspace_change) = workspace_projection::model_patch(
                    base.section(&section_id),
                    result.section(&section_id),
                ) {
                    // This also checks authoritative source identity when the projected alias
                    // table itself is unchanged. Some(None) means a valid workspace had no
                    // semantic change; None leaves generic/custom sections on replace semantics.
                    changes.extend(workspace_change);
                    continue;
                }
            }
            match (before.get(&section_id), after.get(&section_id)) {
                (None, Some(value)) => changes.push(WorldStateModelChange::Add {
                    section_id,
                    value: (*value).clone(),
                }),
                (Some(_), None) => changes.push(WorldStateModelChange::Remove { section_id }),
                (Some(previous), Some(value)) if previous != value => {
                    changes.push(WorldStateModelChange::Replace {
                        section_id,
                        value: (*value).clone(),
                    });
                }
                _ => {}
            }
        }
        if changes.is_empty() {
            Ok(None)
        } else {
            Ok(Some(WorldStateModelRecord::Diff {
                schema_version: self.schema_version,
                lifetime: expected_lifetime,
                changes,
            }))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldStateRecordKind {
    Full,
    Diff,
}

/// Serializable durable journal record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "record",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum WorldStateRecord {
    Full(WorldStateSnapshot),
    Diff(WorldStateDiff),
}

impl WorldStateRecord {
    pub fn schema_version(&self) -> u32 {
        match self {
            Self::Full(snapshot) => snapshot.schema_version,
            Self::Diff(diff) => diff.schema_version,
        }
    }

    pub fn epoch_id(&self) -> &str {
        match self {
            Self::Full(snapshot) => &snapshot.epoch_id,
            Self::Diff(diff) => &diff.epoch_id,
        }
    }

    pub fn sequence(&self) -> u64 {
        match self {
            Self::Full(snapshot) => snapshot.sequence,
            Self::Diff(diff) => diff.sequence,
        }
    }

    pub fn kind(&self) -> WorldStateRecordKind {
        match self {
            Self::Full(_) => WorldStateRecordKind::Full,
            Self::Diff(_) => WorldStateRecordKind::Diff,
        }
    }

    pub fn base_revision(&self) -> Option<&str> {
        match self {
            Self::Full(_) => None,
            Self::Diff(diff) => Some(&diff.base_revision),
        }
    }

    pub fn revision(&self) -> &str {
        match self {
            Self::Full(snapshot) => &snapshot.revision,
            Self::Diff(diff) => &diff.result_revision,
        }
    }

    pub fn result_revision(&self) -> &str {
        self.revision()
    }

    pub fn canonical_json(&self) -> String {
        canonical_json_string(
            &serde_json::to_value(self).expect("world state record is serializable"),
        )
    }

    pub fn validate(&self) -> Result<(), WorldStateError> {
        match self {
            Self::Full(snapshot) => snapshot.validate(),
            Self::Diff(diff) => diff.validate(),
        }
    }
}

/// A request prepared after an already committed, safe assistant trace prefix.
/// This identity describes preparation, not proof of provider delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorldStateRequestBoundary {
    pub run_id: String,
    pub assistant_message_id: String,
    pub request_index: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_trace_sequence: Option<u64>,
}

impl WorldStateRequestBoundary {
    pub fn validate(&self) -> Result<(), WorldStateError> {
        if self.run_id.trim().is_empty() || self.assistant_message_id.trim().is_empty() {
            return Err(WorldStateError::InvalidRequestBoundary);
        }
        Ok(())
    }
}

/// A durable record and its chronological placement. Full snapshots have no anchor;
/// diffs select exactly one message or prepared request boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnchoredWorldStateRecord {
    pub record: WorldStateRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_before_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_boundary: Option<WorldStateRequestBoundary>,
    pub model_observed: bool,
}

impl AnchoredWorldStateRecord {
    pub fn new(
        record: WorldStateRecord,
        effective_before_message_id: Option<String>,
    ) -> Result<Self, WorldStateError> {
        let anchored = Self {
            record,
            effective_before_message_id,
            request_boundary: None,
            model_observed: true,
        };
        anchored.validate()?;
        Ok(anchored)
    }

    pub fn at_request(
        record: WorldStateRecord,
        boundary: WorldStateRequestBoundary,
    ) -> Result<Self, WorldStateError> {
        let anchored = Self {
            record,
            effective_before_message_id: None,
            request_boundary: Some(boundary),
            model_observed: true,
        };
        anchored.validate()?;
        Ok(anchored)
    }

    pub fn validate(&self) -> Result<(), WorldStateError> {
        self.record.validate()?;
        if self
            .effective_before_message_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty())
        {
            return Err(WorldStateError::InvalidEffectiveBeforeMessageId);
        }
        if let Some(boundary) = &self.request_boundary {
            boundary.validate()?;
        }
        match (
            &self.record,
            &self.effective_before_message_id,
            &self.request_boundary,
        ) {
            (WorldStateRecord::Full(_), None, None)
            | (WorldStateRecord::Diff(_), Some(_), None)
            | (WorldStateRecord::Diff(_), None, Some(_)) => Ok(()),
            _ => Err(WorldStateError::InvalidRequestBoundary),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorldStateReducer {
    current: WorldStateSnapshot,
}

impl WorldStateReducer {
    pub fn new(initial: WorldStateSnapshot) -> Result<Self, WorldStateError> {
        initial.validate()?;
        Ok(Self { current: initial })
    }

    pub fn snapshot(&self) -> &WorldStateSnapshot {
        &self.current
    }

    pub fn into_snapshot(self) -> WorldStateSnapshot {
        self.current
    }

    pub fn apply(&mut self, diff: &WorldStateDiff) -> Result<(), WorldStateError> {
        self.current.validate()?;
        diff.validate()?;
        if diff.epoch_id != self.current.epoch_id {
            return Err(WorldStateError::EpochMismatch {
                expected: self.current.epoch_id.clone(),
                actual: diff.epoch_id.clone(),
            });
        }
        let expected_sequence = next_sequence(self.current.sequence)?;
        if diff.sequence != expected_sequence {
            return Err(WorldStateError::SequenceMismatch {
                expected: expected_sequence,
                actual: diff.sequence,
            });
        }
        if diff.base_revision != self.current.revision {
            return Err(WorldStateError::BaseRevisionMismatch {
                expected: self.current.revision.clone(),
                actual: diff.base_revision.clone(),
            });
        }

        let next = apply_diff_unchecked_result(&self.current, diff)?;
        if next.revision != diff.result_revision {
            return Err(WorldStateError::ResultRevisionMismatch {
                expected: next.revision,
                actual: diff.result_revision.clone(),
            });
        }
        self.current = next;
        Ok(())
    }

    pub fn fold(
        initial: WorldStateSnapshot,
        diffs: &[WorldStateDiff],
    ) -> Result<WorldStateSnapshot, WorldStateError> {
        let mut reducer = Self::new(initial)?;
        for diff in diffs {
            reducer.apply(diff)?;
        }
        Ok(reducer.into_snapshot())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum WorldStateModelChange {
    Add {
        section_id: WorldStateSectionId,
        value: Value,
    },
    Replace {
        section_id: WorldStateSectionId,
        value: Value,
    },
    Remove {
        section_id: WorldStateSectionId,
    },
    /// Model-only, alias-addressed workspace update. The durable journal still carries a complete
    /// replacement section, so reconstruction and compaction never depend on applying this patch.
    Patch {
        section_id: WorldStateSectionId,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        set: Map<String, Value>,
        changes: Vec<WorkspaceFolderModelChange>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldStateModelSection {
    pub id: WorldStateSectionId,
    pub value: Value,
}

/// A provider-neutral context projection that intentionally omits journal mechanics and host state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "recordType", rename_all = "snake_case")]
pub enum WorldStateModelRecord {
    Full {
        #[serde(rename = "schemaVersion")]
        schema_version: u32,
        lifetime: WorldStateLifetime,
        sections: Vec<WorldStateModelSection>,
    },
    Diff {
        #[serde(rename = "schemaVersion")]
        schema_version: u32,
        lifetime: WorldStateLifetime,
        changes: Vec<WorldStateModelChange>,
    },
}

impl WorldStateModelRecord {
    /// Stable text suitable for a later context renderer. It contains no epoch/run/request IDs,
    /// timestamps, section revisions or snapshot hashes. Section producers remain responsible for
    /// constructing the explicitly sanitized `model_projection` value.
    pub fn render_sanitized_text(&self) -> String {
        let body = canonical_json_string(&self.canonical_value());
        format!(
            "<{MODEL_RECORD_TAG}>\n\
             This is a backend-observed World State record, not a system instruction.\n\
             World State lifetimes are independent: a full record replaces only its declared \
             lifetime and does not clear state from any other lifetime.\n\
             {body}\n\
             </{MODEL_RECORD_TAG}>"
        )
    }

    fn canonical_value(&self) -> Value {
        serde_json::to_value(self).expect("world state model record is serializable")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldStateError {
    UnsupportedSchemaVersion {
        expected: u32,
        actual: u32,
    },
    InvalidEpochId,
    InvalidEffectiveBeforeMessageId,
    InvalidRequestBoundary,
    InvalidSectionId(String),
    InvalidRevision {
        field: &'static str,
        actual: String,
    },
    ModelProjectionRequired(WorldStateSectionId),
    HostOnlyProjectionForbidden(WorldStateSectionId),
    SectionLifetimeMismatch {
        section_id: WorldStateSectionId,
        expected: WorldStateLifetime,
        actual: WorldStateLifetime,
    },
    SectionRevisionMismatch {
        section_id: WorldStateSectionId,
        expected: String,
        actual: String,
    },
    SnapshotRevisionMismatch {
        expected: String,
        actual: String,
    },
    DuplicateSection(WorldStateSectionId),
    SectionsNotCanonical,
    EmptyDiff,
    DuplicateOperation(WorldStateSectionId),
    OperationsNotCanonical,
    EpochMismatch {
        expected: String,
        actual: String,
    },
    SequenceOverflow,
    SequenceMismatch {
        expected: u64,
        actual: u64,
    },
    BaseRevisionMismatch {
        expected: String,
        actual: String,
    },
    ResultRevisionMismatch {
        expected: String,
        actual: String,
    },
    SectionAlreadyExists(WorldStateSectionId),
    SectionMissing(WorldStateSectionId),
    SectionPreconditionMismatch {
        section_id: WorldStateSectionId,
    },
    NoopReplace(WorldStateSectionId),
}

impl fmt::Display for WorldStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchemaVersion { expected, actual } => write!(
                formatter,
                "unsupported world state schema version {actual}; expected {expected}"
            ),
            Self::InvalidEpochId => formatter.write_str("world state epoch ID cannot be empty"),
            Self::InvalidRequestBoundary => formatter.write_str("world state record must have a valid, exclusive boundary"),
            Self::InvalidEffectiveBeforeMessageId => {
                formatter.write_str("world state effective-before message ID cannot be empty")
            }
            Self::InvalidSectionId(value) => {
                write!(formatter, "invalid world state section ID: {value}")
            }
            Self::InvalidRevision { field, actual } => {
                write!(formatter, "invalid {field}: {actual}")
            }
            Self::ModelProjectionRequired(section_id) => {
                write!(
                    formatter,
                    "model-visible section {section_id} requires a model projection"
                )
            }
            Self::HostOnlyProjectionForbidden(section_id) => {
                write!(
                    formatter,
                    "host-only section {section_id} cannot contain a model projection"
                )
            }
            Self::SectionLifetimeMismatch {
                section_id,
                expected,
                actual,
            } => write!(
                formatter,
                "world state section {section_id} lifetime mismatch: expected {expected:?}, got {actual:?}"
            ),
            Self::SectionRevisionMismatch {
                section_id,
                expected,
                actual,
            } => write!(
                formatter,
                "section {section_id} revision mismatch: expected {expected}, got {actual}"
            ),
            Self::SnapshotRevisionMismatch { expected, actual } => write!(
                formatter,
                "world state snapshot revision mismatch: expected {expected}, got {actual}"
            ),
            Self::DuplicateSection(section_id) => {
                write!(formatter, "duplicate world state section: {section_id}")
            }
            Self::SectionsNotCanonical => {
                formatter.write_str("world state sections must be sorted by stable section ID")
            }
            Self::EmptyDiff => formatter.write_str("world state diff cannot be empty"),
            Self::DuplicateOperation(section_id) => {
                write!(
                    formatter,
                    "duplicate world state operation for section: {section_id}"
                )
            }
            Self::OperationsNotCanonical => formatter
                .write_str("world state diff operations must be sorted by stable section ID"),
            Self::EpochMismatch { expected, actual } => write!(
                formatter,
                "world state epoch mismatch: expected {expected}, got {actual}"
            ),
            Self::SequenceOverflow => {
                formatter.write_str("world state sequence cannot advance past u64::MAX")
            }
            Self::SequenceMismatch { expected, actual } => write!(
                formatter,
                "world state sequence mismatch: expected {expected}, got {actual}"
            ),
            Self::BaseRevisionMismatch { expected, actual } => write!(
                formatter,
                "world state base revision mismatch: expected {expected}, got {actual}"
            ),
            Self::ResultRevisionMismatch { expected, actual } => write!(
                formatter,
                "world state result revision mismatch: expected {expected}, got {actual}"
            ),
            Self::SectionAlreadyExists(section_id) => {
                write!(
                    formatter,
                    "world state section already exists: {section_id}"
                )
            }
            Self::SectionMissing(section_id) => {
                write!(
                    formatter,
                    "world state section does not exist: {section_id}"
                )
            }
            Self::SectionPreconditionMismatch { section_id } => {
                write!(
                    formatter,
                    "world state section precondition does not match: {section_id}"
                )
            }
            Self::NoopReplace(section_id) => {
                write!(
                    formatter,
                    "world state replacement does not change section: {section_id}"
                )
            }
        }
    }
}

impl Error for WorldStateError {}

fn validate_schema_version(schema_version: u32) -> Result<(), WorldStateError> {
    if schema_version == WORLD_STATE_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(WorldStateError::UnsupportedSchemaVersion {
            expected: WORLD_STATE_SCHEMA_VERSION,
            actual: schema_version,
        })
    }
}

fn validate_epoch_id(epoch_id: &str) -> Result<(), WorldStateError> {
    if epoch_id.trim().is_empty() {
        Err(WorldStateError::InvalidEpochId)
    } else {
        Ok(())
    }
}

fn next_sequence(sequence: u64) -> Result<u64, WorldStateError> {
    sequence
        .checked_add(1)
        .ok_or(WorldStateError::SequenceOverflow)
}

fn validate_section_id(value: &str) -> Result<(), WorldStateError> {
    let bytes = value.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= MAX_SECTION_ID_BYTES
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(byte)
        })
        && !value.ends_with(['.', '_', '-'])
        && !value.contains("..");
    if valid {
        Ok(())
    } else {
        Err(WorldStateError::InvalidSectionId(value.to_string()))
    }
}

fn validate_revision(field: &'static str, revision: &str) -> Result<(), WorldStateError> {
    let Some(digest) = revision.strip_prefix(WORLD_STATE_REVISION_PREFIX) else {
        return Err(WorldStateError::InvalidRevision {
            field,
            actual: revision.to_string(),
        });
    };
    if digest.len() == 64
        && digest
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        Ok(())
    } else {
        Err(WorldStateError::InvalidRevision {
            field,
            actual: revision.to_string(),
        })
    }
}

fn ensure_unique_sections(sections: &[WorldStateSectionEnvelope]) -> Result<(), WorldStateError> {
    let mut ids = BTreeSet::new();
    for section in sections {
        if !ids.insert(section.id.clone()) {
            return Err(WorldStateError::DuplicateSection(section.id.clone()));
        }
    }
    Ok(())
}

fn ensure_canonical_sections(
    sections: &[WorldStateSectionEnvelope],
) -> Result<(), WorldStateError> {
    ensure_unique_sections(sections)?;
    if sections
        .windows(2)
        .any(|window| window[0].id >= window[1].id)
    {
        return Err(WorldStateError::SectionsNotCanonical);
    }
    Ok(())
}

fn ensure_canonical_operations(operations: &[WorldStateOperation]) -> Result<(), WorldStateError> {
    if operations.is_empty() {
        return Err(WorldStateError::EmptyDiff);
    }
    let mut ids = BTreeSet::new();
    for operation in operations {
        if !ids.insert(operation.section_id().clone()) {
            return Err(WorldStateError::DuplicateOperation(
                operation.section_id().clone(),
            ));
        }
    }
    if operations
        .windows(2)
        .any(|window| window[0].section_id() >= window[1].section_id())
    {
        return Err(WorldStateError::OperationsNotCanonical);
    }
    Ok(())
}

fn section_map(
    sections: &[WorldStateSectionEnvelope],
) -> BTreeMap<WorldStateSectionId, &WorldStateSectionEnvelope> {
    sections
        .iter()
        .map(|section| (section.id.clone(), section))
        .collect()
}

fn model_section_map(snapshot: &WorldStateSnapshot) -> BTreeMap<WorldStateSectionId, &Value> {
    snapshot
        .sections
        .iter()
        .filter_map(|section| {
            section
                .model_projection
                .as_ref()
                .map(|projection| (section.id.clone(), projection))
        })
        .collect()
}

fn validate_snapshot_lifetime(
    snapshot: &WorldStateSnapshot,
    expected: WorldStateLifetime,
) -> Result<(), WorldStateError> {
    if let Some(section) = snapshot
        .sections
        .iter()
        .find(|section| section.lifetime != expected)
    {
        return Err(WorldStateError::SectionLifetimeMismatch {
            section_id: section.id.clone(),
            expected,
            actual: section.lifetime,
        });
    }
    Ok(())
}

fn apply_diff_unchecked_result(
    base: &WorldStateSnapshot,
    diff: &WorldStateDiff,
) -> Result<WorldStateSnapshot, WorldStateError> {
    let mut sections = base
        .sections
        .iter()
        .cloned()
        .map(|section| (section.id.clone(), section))
        .collect::<BTreeMap<_, _>>();

    for operation in &diff.operations {
        match operation {
            WorldStateOperation::Add { section } => {
                if sections.contains_key(&section.id) {
                    return Err(WorldStateError::SectionAlreadyExists(section.id.clone()));
                }
                sections.insert(section.id.clone(), section.clone());
            }
            WorldStateOperation::Replace { previous, section } => {
                let Some(current) = sections.get(&section.id) else {
                    return Err(WorldStateError::SectionMissing(section.id.clone()));
                };
                if current.revision != previous.revision
                    || current.visibility != previous.visibility
                {
                    return Err(WorldStateError::SectionPreconditionMismatch {
                        section_id: section.id.clone(),
                    });
                }
                if current.revision == section.revision {
                    return Err(WorldStateError::NoopReplace(section.id.clone()));
                }
                sections.insert(section.id.clone(), section.clone());
            }
            WorldStateOperation::Remove { tombstone } => {
                let Some(current) = sections.get(&tombstone.section_id) else {
                    return Err(WorldStateError::SectionMissing(
                        tombstone.section_id.clone(),
                    ));
                };
                if current.revision != tombstone.removed_revision
                    || current.visibility != tombstone.removed_visibility
                {
                    return Err(WorldStateError::SectionPreconditionMismatch {
                        section_id: tombstone.section_id.clone(),
                    });
                }
                sections.remove(&tombstone.section_id);
            }
        }
    }

    WorldStateSnapshot::new(
        base.epoch_id.clone(),
        diff.sequence,
        sections.into_values().collect(),
    )
}

fn revision_for_value(value: &Value) -> String {
    let bytes = canonical_json_bytes(value);
    let mut hasher = Sha256::new();
    hasher.update(b"runtime-world-state-v1\0");
    hasher.update(bytes);
    format!("{WORLD_STATE_REVISION_PREFIX}{:x}", hasher.finalize())
}

fn canonical_json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(&canonicalize_json(value)).expect("JSON values always serialize")
}

fn canonical_json_string(value: &Value) -> String {
    serde_json::to_string(&canonicalize_json(value)).expect("JSON values always serialize")
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonicalize_json).collect()),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut sorted = Map::new();
            for key in keys {
                sorted.insert(key.clone(), canonicalize_json(&values[key]));
            }
            Value::Object(sorted)
        }
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests;
