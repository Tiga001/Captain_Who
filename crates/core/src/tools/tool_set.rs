use super::{AgentToolExposure, ToolRegistry};
use crate::protocol::{
    AgentError, AgentResult, AgentRunToolSetCheckpoint, AgentToolDefinition, AgentToolIdentity,
};
use crate::revision::content_revision;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

const TOOL_CAPABILITY_ID_MAX_BYTES: usize = 128;
const TOOL_SET_REVISION_SCHEMA_VERSION: u32 = 2;

pub(crate) const OFFICE_DOCUMENTS_CAPABILITY: &str = "office.documents";
pub(crate) const OFFICE_SPREADSHEETS_CAPABILITY: &str = "office.spreadsheets";
pub(crate) const OFFICE_PRESENTATIONS_CAPABILITY: &str = "office.presentations";
pub(crate) const IMAGE_GENERATION_CAPABILITY: &str = "image.generation";
pub(crate) const SKILL_RESOURCES_READ_CAPABILITY: &str = "skill.resources.read";
pub(crate) const SKILL_RESOURCES_MATERIALIZE_CAPABILITY: &str = "skill.resources.materialize";
pub(crate) const SKILL_SCRIPTS_CAPABILITY: &str = "skill.scripts";
pub(crate) const SKILL_INSTALLATION_CAPABILITY: &str = "skill.installation";

/// Backend-owned identifier that connects an activated capability to a registered Tool.
///
/// Capability ids are never accepted from model arguments. They are derived from verified Skill
/// identities and resource manifests, then matched against application-owned Tool registrations.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub(crate) struct ToolCapabilityId(String);

impl ToolCapabilityId {
    pub(crate) fn parse(value: impl Into<String>) -> AgentResult<Self> {
        let value = value.into();
        if !valid_capability_id(&value) {
            return Err(AgentError::new(format!(
                "工具能力 id `{value}` 无效；必须是最多 {TOOL_CAPABILITY_ID_MAX_BYTES} 字节的小写命名空间标识。"
            )));
        }
        Ok(Self(value))
    }

    pub(crate) fn application_owned(value: &'static str) -> Self {
        Self::parse(value).expect("application-owned Tool capability ids must be valid")
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

fn valid_capability_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= TOOL_CAPABILITY_ID_MAX_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && !value.ends_with(['.', '_', '-'])
        && !value.contains("..")
}

/// Immutable model-facing Tool contract for one request boundary.
///
/// The private [`ToolRegistry`] may contain more implementations than this set. Stable definitions
/// always form an exact prefix; dynamically unlocked definitions are sorted independently and
/// appended afterwards. `exposed_names` is the execution allowlist for calls produced by this
/// exact contract.
#[derive(Debug, Clone)]
pub(crate) struct EffectiveToolSet {
    stable_definitions: Vec<AgentToolDefinition>,
    dynamic_definitions: Vec<AgentToolDefinition>,
    exposed_names: BTreeSet<String>,
    permitted_definitions: BTreeMap<String, AgentToolDefinition>,
    permitted_names: BTreeSet<String>,
    registered_exposures: BTreeMap<String, AgentToolExposure>,
    registered_identities: BTreeMap<String, AgentToolIdentity>,
    active_capabilities: BTreeSet<ToolCapabilityId>,
    stable_revision: String,
    dynamic_revision: String,
    revision: String,
}

/// Backend-authoritative explanation for why a Tool omitted from the effective request contract
/// cannot be executed.
///
/// The model never supplies this state. It is derived from the frozen registry, permission-filtered
/// definitions, and verified active Skill capabilities used to build the exact request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolUnavailability {
    RequiresSkillActivation {
        required_capability: ToolCapabilityId,
    },
    RequiresBuiltinCapabilityActivation {
        required_capability: ToolCapabilityId,
    },
    BlockedByPermissions,
    RuntimeCapabilityUnavailable {
        required_capability: ToolCapabilityId,
    },
    NotRegistered,
}

impl EffectiveToolSet {
    pub(crate) fn from_permitted_definitions(
        registry: &ToolRegistry,
        permitted_definitions: impl IntoIterator<Item = AgentToolDefinition>,
        active_capabilities: &BTreeSet<ToolCapabilityId>,
    ) -> AgentResult<Self> {
        let mut permitted = BTreeMap::new();
        for definition in permitted_definitions {
            let name = definition.name.clone();
            if registry.exposure(&name).is_none() {
                return Err(AgentError::new(format!(
                    "有效工具集包含未注册工具 `{name}`。"
                )));
            }
            if permitted.insert(name.clone(), definition).is_some() {
                return Err(AgentError::new(format!(
                    "有效工具集包含重复工具 `{name}`。"
                )));
            }
        }

        let mut registered_exposures = BTreeMap::new();
        let mut registered_identities = BTreeMap::new();
        for definition in registry.definitions() {
            let name = definition.name;
            let exposure = registry
                .exposure(&name)
                .expect("registered Tool definitions must have exposure metadata")
                .clone();
            let identity = registry
                .identity(&name)
                .expect("registered Tool definitions must have typed identity metadata")
                .clone();
            registered_exposures.insert(name.clone(), exposure);
            registered_identities.insert(name, identity);
        }

        Self::from_validated_parts(
            permitted,
            registered_exposures,
            registered_identities,
            active_capabilities.clone(),
        )
    }

    fn from_validated_parts(
        permitted_definitions: BTreeMap<String, AgentToolDefinition>,
        registered_exposures: BTreeMap<String, AgentToolExposure>,
        registered_identities: BTreeMap<String, AgentToolIdentity>,
        active_capabilities: BTreeSet<ToolCapabilityId>,
    ) -> AgentResult<Self> {
        let permitted_names = permitted_definitions
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut stable_definitions = Vec::new();
        let mut dynamic_definitions = Vec::new();
        for (name, definition) in &permitted_definitions {
            match registered_exposures
                .get(name)
                .expect("permitted definitions were validated against the registry")
            {
                AgentToolExposure::Stable => stable_definitions.push(definition),
                AgentToolExposure::Dynamic => dynamic_definitions.push(definition),
                AgentToolExposure::RequiresCapability(capability)
                    if active_capabilities.contains(capability) =>
                {
                    dynamic_definitions.push(definition);
                }
                AgentToolExposure::RequiresCapability(_) => {}
            }
        }
        let stable_definitions = stable_definitions.into_iter().cloned().collect::<Vec<_>>();
        let dynamic_definitions = dynamic_definitions.into_iter().cloned().collect::<Vec<_>>();

        // `permitted` is a BTreeMap, so each partition is already sorted by Tool name. Keep the
        // explicit assertions close to construction because stable-prefix ordering is a cache
        // contract, not a presentation detail.
        debug_assert!(strictly_sorted(&stable_definitions));
        debug_assert!(strictly_sorted(&dynamic_definitions));

        let exposed_names = stable_definitions
            .iter()
            .chain(&dynamic_definitions)
            .map(|definition| definition.name.clone())
            .collect::<BTreeSet<_>>();
        let stable_revision = definition_revision(
            "stable",
            &stable_definitions,
            &registered_identities,
            std::iter::empty::<&ToolCapabilityId>(),
        )?;
        let dynamic_revision = definition_revision(
            "dynamic",
            &dynamic_definitions,
            &registered_identities,
            active_capabilities.iter(),
        )?;
        let revision_material = serde_json::to_vec(&(
            TOOL_SET_REVISION_SCHEMA_VERSION,
            &stable_revision,
            &dynamic_revision,
        ))
        .map_err(|error| AgentError::new(format!("无法生成有效工具集 revision：{error}")))?;
        let revision = format!(
            "effective-tool-set-v{TOOL_SET_REVISION_SCHEMA_VERSION}:{}",
            content_revision(&revision_material)
        );

        Ok(Self {
            stable_definitions,
            dynamic_definitions,
            exposed_names,
            permitted_definitions,
            permitted_names,
            registered_exposures,
            registered_identities,
            active_capabilities,
            stable_revision,
            dynamic_revision,
            revision,
        })
    }

    /// Projects the exact model-facing Tool contract after adding backend-verified capabilities.
    ///
    /// The projection reuses the permission-filtered definitions and registry exposure metadata
    /// frozen for this run. It cannot make an unregistered or permission-blocked Tool available.
    /// Skill activation uses this before committing resources or activation state so the next
    /// request's dynamic contract can be included in the same atomic capacity check.
    pub(crate) fn with_additional_capabilities(
        &self,
        additional_capabilities: &BTreeSet<ToolCapabilityId>,
    ) -> AgentResult<Self> {
        let mut active_capabilities = self.active_capabilities.clone();
        active_capabilities.extend(additional_capabilities.iter().cloned());
        Self::from_validated_parts(
            self.permitted_definitions.clone(),
            self.registered_exposures.clone(),
            self.registered_identities.clone(),
            active_capabilities,
        )
    }

    pub(crate) fn stable_definitions(&self) -> &[AgentToolDefinition] {
        &self.stable_definitions
    }

    pub(crate) fn dynamic_definitions(&self) -> &[AgentToolDefinition] {
        &self.dynamic_definitions
    }

    pub(crate) fn all_definitions(&self) -> Vec<AgentToolDefinition> {
        self.stable_definitions
            .iter()
            .chain(&self.dynamic_definitions)
            .cloned()
            .collect()
    }

    pub(crate) fn contains(&self, tool_name: &str) -> bool {
        self.exposed_names.contains(tool_name)
    }

    pub(crate) fn unavailability(&self, tool_name: &str) -> Option<ToolUnavailability> {
        if self.contains(tool_name) {
            return None;
        }

        let registered_exposure = self.registered_exposures.get(tool_name);
        let expected_capability = match registered_exposure {
            Some(AgentToolExposure::RequiresCapability(capability)) => Some(capability.clone()),
            _ => application_owned_dynamic_capability(tool_name),
        };

        if let Some(required_capability) = expected_capability {
            if !self.active_capabilities.contains(&required_capability) {
                if matches!(
                    self.registered_identities.get(tool_name),
                    Some(AgentToolIdentity::BuiltinCapability { .. })
                ) {
                    return Some(ToolUnavailability::RequiresBuiltinCapabilityActivation {
                        required_capability,
                    });
                }
                return Some(ToolUnavailability::RequiresSkillActivation {
                    required_capability,
                });
            }
            if registered_exposure.is_none() {
                return Some(ToolUnavailability::RuntimeCapabilityUnavailable {
                    required_capability,
                });
            }
        }

        if registered_exposure.is_some() && !self.permitted_names.contains(tool_name) {
            return Some(ToolUnavailability::BlockedByPermissions);
        }

        Some(ToolUnavailability::NotRegistered)
    }

    pub(crate) fn stable_revision(&self) -> &str {
        &self.stable_revision
    }

    pub(crate) fn dynamic_revision(&self) -> &str {
        &self.dynamic_revision
    }

    pub(crate) fn revision(&self) -> &str {
        &self.revision
    }

    pub(crate) fn checkpoint(&self) -> AgentRunToolSetCheckpoint {
        AgentRunToolSetCheckpoint {
            stable_revision: self.stable_revision.clone(),
            dynamic_revision: self.dynamic_revision.clone(),
            effective_revision: self.revision.clone(),
            active_capability_ids: self
                .active_capabilities
                .iter()
                .map(|capability| capability.as_str().to_string())
                .collect(),
            exposed_tool_names: self.exposed_names.iter().cloned().collect(),
        }
    }

    /// Rebuilds the exact request-boundary contract for a paused Tool batch from the current
    /// trusted registry and permission-filtered definitions.
    ///
    /// Extension snapshots are intentionally allowed to be newer: a preceding sibling may have
    /// successfully activated a Skill before a later sibling paused for Approval. The checkpoint
    /// capability set must still be a subset of that restored extension authority, and every
    /// revision/name is recomputed before any queued call can execute.
    pub(crate) fn restore_frozen_checkpoint(
        &self,
        checkpoint: &AgentRunToolSetCheckpoint,
    ) -> AgentResult<Self> {
        validate_tool_set_checkpoint_shape(checkpoint)?;
        let frozen_capabilities = checkpoint
            .active_capability_ids
            .iter()
            .cloned()
            .map(ToolCapabilityId::parse)
            .collect::<AgentResult<BTreeSet<_>>>()?;
        if !frozen_capabilities.is_subset(&self.active_capabilities) {
            return Err(tool_set_checkpoint_mismatch(checkpoint, self));
        }
        let frozen = Self::from_validated_parts(
            self.permitted_definitions.clone(),
            self.registered_exposures.clone(),
            self.registered_identities.clone(),
            frozen_capabilities,
        )?;
        frozen.validate_checkpoint(checkpoint)?;
        Ok(frozen)
    }

    pub(crate) fn validate_checkpoint(
        &self,
        checkpoint: &AgentRunToolSetCheckpoint,
    ) -> AgentResult<()> {
        validate_tool_set_checkpoint_shape(checkpoint)?;
        if checkpoint.stable_revision != self.stable_revision
            || checkpoint.dynamic_revision != self.dynamic_revision
            || checkpoint.effective_revision != self.revision
            || checkpoint
                .active_capability_ids
                .iter()
                .map(String::as_str)
                .ne(self
                    .active_capabilities
                    .iter()
                    .map(ToolCapabilityId::as_str))
            || checkpoint
                .exposed_tool_names
                .iter()
                .ne(self.exposed_names.iter())
        {
            return Err(tool_set_checkpoint_mismatch(checkpoint, self));
        }
        Ok(())
    }
}

fn tool_set_checkpoint_mismatch(
    checkpoint: &AgentRunToolSetCheckpoint,
    actual: &EffectiveToolSet,
) -> AgentError {
    AgentError::structured(
        "agent.checkpoint_tool_set_mismatch",
        "无法恢复运行检查点：当前工具集与暂停时冻结的工具集不一致。",
        serde_json::json!({
            "type": "checkpoint",
            "code": "toolSetMismatch",
            "recovery": "restartRun",
            "expectedStableRevision": checkpoint.stable_revision,
            "actualStableRevision": actual.stable_revision,
            "expectedDynamicRevision": checkpoint.dynamic_revision,
            "actualDynamicRevision": actual.dynamic_revision,
        }),
    )
}

/// Capability expectations for application-owned dynamic Tools whose concrete implementation is
/// registered only when its Host service is ready. Keeping this mapping beside the effective-set
/// policy prevents an absent Office engine or image provider from being misreported as an
/// unactivated Skill after that Skill is already active.
fn application_owned_dynamic_capability(tool_name: &str) -> Option<ToolCapabilityId> {
    let capability = match tool_name {
        "read_word" | "office_document" => OFFICE_DOCUMENTS_CAPABILITY,
        "read_spreadsheet" | "office_spreadsheet" => OFFICE_SPREADSHEETS_CAPABILITY,
        "read_presentation" | "office_presentation" => OFFICE_PRESENTATIONS_CAPABILITY,
        "image_generation" => IMAGE_GENERATION_CAPABILITY,
        "skills_list_resources" | "skills_read_resource" => SKILL_RESOURCES_READ_CAPABILITY,
        "skills_materialize_resource" => SKILL_RESOURCES_MATERIALIZE_CAPABILITY,
        "skills_preflight_script" | "skills_run_script" => SKILL_SCRIPTS_CAPABILITY,
        "skills_prepare_install" | "skills_commit_install" => SKILL_INSTALLATION_CAPABILITY,
        _ => return None,
    };
    Some(ToolCapabilityId::application_owned(capability))
}

pub(crate) fn validate_tool_set_checkpoint_shape(
    checkpoint: &AgentRunToolSetCheckpoint,
) -> AgentResult<()> {
    if checkpoint.stable_revision.trim().is_empty()
        || checkpoint.dynamic_revision.trim().is_empty()
        || checkpoint.effective_revision.trim().is_empty()
        || checkpoint
            .active_capability_ids
            .iter()
            .any(|capability| ToolCapabilityId::parse(capability.clone()).is_err())
        || !checkpoint
            .active_capability_ids
            .windows(2)
            .all(|pair| pair[0] < pair[1])
        || checkpoint
            .exposed_tool_names
            .iter()
            .any(|name| name.trim().is_empty() || name.trim() != name)
        || !checkpoint
            .exposed_tool_names
            .windows(2)
            .all(|pair| pair[0] < pair[1])
    {
        return Err(AgentError::new(
            "无法恢复运行检查点：冻结工具集缺少 revision，或 capability/tool 不是有序唯一集合。",
        ));
    }
    Ok(())
}

fn strictly_sorted(definitions: &[AgentToolDefinition]) -> bool {
    definitions
        .windows(2)
        .all(|pair| pair[0].name < pair[1].name)
}

fn definition_revision<'a>(
    partition: &str,
    definitions: &[AgentToolDefinition],
    registered_identities: &BTreeMap<String, AgentToolIdentity>,
    capabilities: impl IntoIterator<Item = &'a ToolCapabilityId>,
) -> AgentResult<String> {
    let capabilities = capabilities
        .into_iter()
        .map(ToolCapabilityId::as_str)
        .collect::<Vec<_>>();
    let identities = definitions
        .iter()
        .map(|definition| {
            registered_identities
                .get(&definition.name)
                .map(|identity| (&definition.name, identity))
                .ok_or_else(|| {
                    AgentError::new(format!(
                        "工具 `{}` 缺少 revision 所需的稳定身份。",
                        definition.name
                    ))
                })
        })
        .collect::<AgentResult<Vec<_>>>()?;
    let material = serde_json::to_vec(&(
        TOOL_SET_REVISION_SCHEMA_VERSION,
        partition,
        capabilities,
        definitions,
        identities,
    ))
    .map_err(|error| AgentError::new(format!("无法生成 {partition} 工具集 revision：{error}")))?;
    Ok(format!(
        "{partition}-tool-set-v{TOOL_SET_REVISION_SCHEMA_VERSION}:{}",
        content_revision(&material)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{AgentToolApprovalMode, AgentToolDefinition, AgentToolSafety};
    use crate::tools::{AgentTool, AgentToolPermissionPolicy, ToolExecutionContext};
    use serde_json::{json, Value};
    use std::sync::Arc;

    struct NoopSkillInstallationPrepare;

    struct NoopSkillInstallationCommit;

    impl crate::tools::AgentSkillInstallationPrepareExecutor for NoopSkillInstallationPrepare {
        fn prepare(
            &self,
            _request: crate::tools::AgentSkillInstallationPrepareRequest,
        ) -> AgentResult<Value> {
            Ok(json!({ "status": "ready" }))
        }
    }

    impl crate::tools::AgentSkillInstallationCommitPreparer for NoopSkillInstallationCommit {
        fn prepare_commit_action(
            &self,
            _request: crate::tools::AgentSkillInstallationCommitPreparationRequest,
        ) -> AgentResult<crate::protocol::AgentSkillInstallationRequest> {
            Err(AgentError::new("not executed in Tool set tests"))
        }

        fn invalidate_commit_action(
            &self,
            _action: &crate::protocol::AgentSkillInstallationRequest,
        ) -> AgentResult<()> {
            Ok(())
        }
    }

    struct TestTool {
        name: &'static str,
        exposure: AgentToolExposure,
    }

    impl AgentTool for TestTool {
        fn definition(&self) -> AgentToolDefinition {
            AgentToolDefinition {
                name: self.name.to_string(),
                description: format!("{} definition", self.name),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
                safety: AgentToolSafety::ReadOnly,
                requires_workspace: false,
                requires_approval: false,
                approval_mode: AgentToolApprovalMode::Never,
            }
        }

        fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
            Ok(json!({}))
        }

        fn permission_policy(&self) -> AgentToolPermissionPolicy {
            AgentToolPermissionPolicy::Default
        }

        fn exposure(&self) -> AgentToolExposure {
            self.exposure.clone()
        }
    }

    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::empty();
        registry.register(TestTool {
            name: "z_stable",
            exposure: AgentToolExposure::Stable,
        });
        registry.register(TestTool {
            name: "a_dynamic",
            exposure: AgentToolExposure::RequiresCapability(ToolCapabilityId::application_owned(
                OFFICE_DOCUMENTS_CAPABILITY,
            )),
        });
        registry.register(TestTool {
            name: "b_stable",
            exposure: AgentToolExposure::Stable,
        });
        registry
    }

    #[test]
    fn stable_definitions_are_an_exact_prefix_and_dynamic_tools_require_capability() {
        let registry = registry();
        let without = EffectiveToolSet::from_permitted_definitions(
            &registry,
            registry.definitions(),
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(
            without
                .all_definitions()
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            vec!["b_stable", "z_stable"]
        );
        assert!(!without.contains("a_dynamic"));

        let capabilities = BTreeSet::from([ToolCapabilityId::application_owned(
            OFFICE_DOCUMENTS_CAPABILITY,
        )]);
        let with = EffectiveToolSet::from_permitted_definitions(
            &registry,
            registry.definitions(),
            &capabilities,
        )
        .unwrap();
        assert_eq!(
            with.all_definitions()
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            vec!["b_stable", "z_stable", "a_dynamic"]
        );
        assert_eq!(without.stable_revision(), with.stable_revision());
        assert_ne!(without.dynamic_revision(), with.dynamic_revision());
        assert_ne!(
            without.checkpoint().effective_revision,
            with.checkpoint().effective_revision
        );
        let projected = without.with_additional_capabilities(&capabilities).unwrap();
        assert_eq!(
            projected
                .all_definitions()
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            vec!["b_stable", "z_stable", "a_dynamic"]
        );
        assert_eq!(projected.stable_revision(), without.stable_revision());
        assert_eq!(projected.dynamic_revision(), with.dynamic_revision());
        assert_eq!(projected.revision(), with.revision());
        with.validate_checkpoint(&with.checkpoint()).unwrap();

        let frozen_before_activation = without.checkpoint();
        let restored_batch = with
            .restore_frozen_checkpoint(&frozen_before_activation)
            .expect("post-effect authority must restore the earlier request ToolSet exactly");
        assert_eq!(restored_batch.checkpoint(), frozen_before_activation);
        assert!(!restored_batch.contains("a_dynamic"));
        assert!(with.contains("a_dynamic"));

        let mut forged_capability = frozen_before_activation.clone();
        forged_capability.active_capability_ids = vec!["skill.forged".to_string()];
        let error = with
            .restore_frozen_checkpoint(&forged_capability)
            .unwrap_err();
        assert_eq!(error.code(), Some("agent.checkpoint_tool_set_mismatch"));

        let mut duplicate_capability = with.checkpoint();
        duplicate_capability.active_capability_ids = vec![
            OFFICE_DOCUMENTS_CAPABILITY.to_string(),
            OFFICE_DOCUMENTS_CAPABILITY.to_string(),
        ];
        assert!(validate_tool_set_checkpoint_shape(&duplicate_capability).is_err());
        let mut unsorted_capabilities = with.checkpoint();
        unsorted_capabilities.active_capability_ids =
            vec!["skill.z".to_string(), "skill.a".to_string()];
        assert!(validate_tool_set_checkpoint_shape(&unsorted_capabilities).is_err());

        let mut tampered = with.checkpoint();
        tampered.exposed_tool_names.pop();
        let error = with.validate_checkpoint(&tampered).unwrap_err();
        assert_eq!(error.code(), Some("agent.checkpoint_tool_set_mismatch"));
    }

    #[test]
    fn inactive_capabilities_and_permission_filtering_do_not_change_stable_revision() {
        let registry = registry();
        let permitted = registry
            .definitions()
            .into_iter()
            .filter(|definition| definition.name != "a_dynamic")
            .collect::<Vec<_>>();
        let inactive = EffectiveToolSet::from_permitted_definitions(
            &registry,
            permitted.clone(),
            &BTreeSet::new(),
        )
        .unwrap();
        let active = EffectiveToolSet::from_permitted_definitions(
            &registry,
            permitted,
            &BTreeSet::from([ToolCapabilityId::application_owned(
                OFFICE_DOCUMENTS_CAPABILITY,
            )]),
        )
        .unwrap();
        assert_eq!(inactive.stable_revision(), active.stable_revision());
        assert!(active.dynamic_definitions().is_empty());
        assert_ne!(inactive.dynamic_revision(), active.dynamic_revision());
        assert_eq!(
            inactive.unavailability("a_dynamic"),
            Some(ToolUnavailability::RequiresSkillActivation {
                required_capability: ToolCapabilityId::application_owned(
                    OFFICE_DOCUMENTS_CAPABILITY
                ),
            })
        );
        assert_eq!(
            active.unavailability("a_dynamic"),
            Some(ToolUnavailability::BlockedByPermissions)
        );
    }

    #[test]
    fn missing_host_implementation_is_not_misreported_after_skill_activation() {
        let registry = registry();
        let inactive = EffectiveToolSet::from_permitted_definitions(
            &registry,
            registry.definitions(),
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(
            inactive.unavailability("office_document"),
            Some(ToolUnavailability::RequiresSkillActivation {
                required_capability: ToolCapabilityId::application_owned(
                    OFFICE_DOCUMENTS_CAPABILITY
                ),
            })
        );

        let active = EffectiveToolSet::from_permitted_definitions(
            &registry,
            registry.definitions(),
            &BTreeSet::from([ToolCapabilityId::application_owned(
                OFFICE_DOCUMENTS_CAPABILITY,
            )]),
        )
        .unwrap();
        assert_eq!(
            active.unavailability("office_document"),
            Some(ToolUnavailability::RuntimeCapabilityUnavailable {
                required_capability: ToolCapabilityId::application_owned(
                    OFFICE_DOCUMENTS_CAPABILITY
                ),
            })
        );
        assert_eq!(
            active.unavailability("invented_tool"),
            Some(ToolUnavailability::NotRegistered)
        );
        assert_eq!(active.unavailability("b_stable"), None);
    }

    #[test]
    fn skill_installation_tool_is_absent_until_both_host_and_capability_are_available() {
        let mut registered = ToolRegistry::empty();
        registered.register_skill_installation_prepare(Arc::new(NoopSkillInstallationPrepare));
        registered.register_skill_installation_commit(Arc::new(NoopSkillInstallationCommit));
        let inactive = EffectiveToolSet::from_permitted_definitions(
            &registered,
            registered.definitions(),
            &BTreeSet::new(),
        )
        .unwrap();
        assert!(!inactive.contains("skills_prepare_install"));
        assert!(!inactive.contains("skills_commit_install"));
        assert_eq!(
            inactive.unavailability("skills_prepare_install"),
            Some(ToolUnavailability::RequiresSkillActivation {
                required_capability: ToolCapabilityId::application_owned(
                    SKILL_INSTALLATION_CAPABILITY,
                ),
            })
        );

        let capability = BTreeSet::from([ToolCapabilityId::application_owned(
            SKILL_INSTALLATION_CAPABILITY,
        )]);
        let active = inactive.with_additional_capabilities(&capability).unwrap();
        assert!(active.contains("skills_prepare_install"));
        assert!(active.contains("skills_commit_install"));
        assert_eq!(active.dynamic_definitions().len(), 2);
        active.validate_checkpoint(&active.checkpoint()).unwrap();

        let missing_host = EffectiveToolSet::from_permitted_definitions(
            &ToolRegistry::empty(),
            Vec::new(),
            &capability,
        )
        .unwrap();
        assert_eq!(
            missing_host.unavailability("skills_prepare_install"),
            Some(ToolUnavailability::RuntimeCapabilityUnavailable {
                required_capability: ToolCapabilityId::application_owned(
                    SKILL_INSTALLATION_CAPABILITY,
                ),
            })
        );
        assert_eq!(
            missing_host.unavailability("skills_commit_install"),
            Some(ToolUnavailability::RuntimeCapabilityUnavailable {
                required_capability: ToolCapabilityId::application_owned(
                    SKILL_INSTALLATION_CAPABILITY,
                ),
            })
        );
    }

    #[test]
    fn rejects_invalid_capability_ids_and_unregistered_definitions() {
        assert!(ToolCapabilityId::parse("Office.Documents").is_err());
        assert!(ToolCapabilityId::parse("office..documents").is_err());
        assert!(ToolCapabilityId::parse("office.documents.").is_err());

        let registry = registry();
        let error = EffectiveToolSet::from_permitted_definitions(
            &registry,
            [AgentToolDefinition {
                name: "unknown".to_string(),
                description: "unknown".to_string(),
                input_schema: json!({"type": "object"}),
                safety: AgentToolSafety::ReadOnly,
                requires_workspace: false,
                requires_approval: false,
                approval_mode: AgentToolApprovalMode::Never,
            }],
            &BTreeSet::new(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("未注册工具"));
    }

    #[test]
    fn default_registry_classifies_core_and_skill_resource_tools_explicitly() {
        let registry = ToolRegistry::defaults_with_search(None);
        let expected = BTreeMap::from([
            ("apply_patch", AgentToolExposure::Stable),
            ("attachments_list", AgentToolExposure::Stable),
            ("attachments_list_project", AgentToolExposure::Stable),
            ("command_session", AgentToolExposure::Stable),
            ("git_diff", AgentToolExposure::Stable),
            ("read_file", AgentToolExposure::Stable),
            ("read_image", AgentToolExposure::Stable),
            (
                "read_presentation",
                AgentToolExposure::RequiresCapability(ToolCapabilityId::application_owned(
                    OFFICE_PRESENTATIONS_CAPABILITY,
                )),
            ),
            (
                "read_spreadsheet",
                AgentToolExposure::RequiresCapability(ToolCapabilityId::application_owned(
                    OFFICE_SPREADSHEETS_CAPABILITY,
                )),
            ),
            (
                "read_word",
                AgentToolExposure::RequiresCapability(ToolCapabilityId::application_owned(
                    OFFICE_DOCUMENTS_CAPABILITY,
                )),
            ),
            ("run_command", AgentToolExposure::Stable),
            ("search_code", AgentToolExposure::Stable),
            ("search_files", AgentToolExposure::Stable),
            (
                "skills_list_resources",
                AgentToolExposure::RequiresCapability(ToolCapabilityId::application_owned(
                    SKILL_RESOURCES_READ_CAPABILITY,
                )),
            ),
            (
                "skills_materialize_resource",
                AgentToolExposure::RequiresCapability(ToolCapabilityId::application_owned(
                    SKILL_RESOURCES_MATERIALIZE_CAPABILITY,
                )),
            ),
            (
                "skills_preflight_script",
                AgentToolExposure::RequiresCapability(ToolCapabilityId::application_owned(
                    SKILL_SCRIPTS_CAPABILITY,
                )),
            ),
            (
                "skills_read_resource",
                AgentToolExposure::RequiresCapability(ToolCapabilityId::application_owned(
                    SKILL_RESOURCES_READ_CAPABILITY,
                )),
            ),
            (
                "skills_run_script",
                AgentToolExposure::RequiresCapability(ToolCapabilityId::application_owned(
                    SKILL_SCRIPTS_CAPABILITY,
                )),
            ),
            ("workspace_map", AgentToolExposure::Stable),
            ("write_file", AgentToolExposure::Stable),
        ])
        .into_iter()
        .map(|(name, exposure)| (name.to_string(), exposure))
        .collect::<BTreeMap<_, _>>();
        let actual = registry
            .definitions()
            .into_iter()
            .map(|definition| {
                let exposure = registry
                    .exposure(&definition.name)
                    .cloned()
                    .expect("every registered Tool must have an explicit exposure contract");
                (definition.name, exposure)
            })
            .collect::<BTreeMap<_, _>>();

        assert_eq!(actual, expected);

        for (name, exposure) in expected {
            assert_eq!(
                registry.exposure(&name),
                Some(&exposure),
                "{name} exposure changed unexpectedly"
            );
        }
    }
}
